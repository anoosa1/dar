# Mail

Optional built-in mail client at `/mail` (administrators can turn it off in
Administration → Apps; data is kept). Layout follows the selected
[direction A with B's inline reply](spacetimedb/mail-mocks.html) (November 22, 2026):
accounts and mailboxes in the sidebar, a day-grouped message list, the conversation in
the right sheet with an inline reply, and a floating compose sheet.

## Accounts and providers

- People add accounts only with **providers the administrator set up**: in the host
  configuration (`services.dar.mail.providers`, below) or in Mail settings → Providers
  (administrators only). A provider names its servers and how people sign in. Mail reaches
  no other servers.
- **Protocols:** IMAP4rev1 with SMTP submission, or JMAP (RFC 8620/8621, for example
  Fastmail or Stalwart). Connections are always TLS: from the start (993, 465) or STARTTLS
  (143, 587). Certificates are checked against the system trust store.
- **Sign-in:** a password or app password, a JMAP API token, or OAuth2 (Google, Microsoft or
  any provider with authorization-code flow and PKCE; IMAP and SMTP then use XOAUTH2). The
  sign-in is checked against the server before an account is saved.
- Passwords, tokens and OAuth grants are sealed on the server (AES-GCM under the instance
  keyring) and never logged or sent to the browser. Administration-made providers keep their
  OAuth client secret sealed the same way; host-configured ones read it from a file at start.
- Up to 20 accounts per person, each with a name, colour, display name and signature.
  Removing an account forgets its sign-in and keeps its `.eml` files unless asked.

## The Files mirror

Every message is kept as its original `.eml` file in Files:
`docs/mail/<account>/<mailbox>/YYYY-MM-DD HHMM Subject.eml` (the date in UTC; mailboxes nest as
on the server). Mail lists and searches from an index of those files and **reads every message
from its file**, never from the server, so mirrored mail stays readable when the server is
down or the account stops syncing. Files' versions, backups, quotas and search apply to it.

Mail checks each account every five minutes (and on Sync now). A pass first carries
changes made **in Files** to the server, then brings the server's changes into Files:

| In Files | On the server |
|---|---|
| An `.eml` moved to another mailbox's folder | The message moves there |
| An `.eml` deleted (moved to deleted files) | The message moves to Trash (from Trash: deleted for good) |
| An `.eml` moved out of the mail folder | Same as deleted; the file stays where you put it |
| An `.eml` edited (a new version) | The new content replaces the message |
| An `.eml` added to a mailbox folder | The message is added (APPEND / Email/import) |
| A folder made in an account or mailbox folder | A new mailbox (with any `.eml` files in it) |
| A mailbox folder renamed | The mailbox is renamed (not Inbox) |
| A whole mailbox or account folder deleted | Nothing: the folder is made again and its messages written again |

| On the server | In Files |
|---|---|
| New message | A new `.eml` file |
| Message removed | Its file moves to deleted files (restorable there) |
| Message moved | Its file moves |
| Flags changed (read, flagged, answered) | Kept in the index (files have no flags) |
| Mailbox removed | Its files move to deleted files |
| UIDVALIDITY changed (IMAP) | The mailbox is mirrored again |

Moving a mailbox folder to another parent in Files is not carried over. IMAP virtual
mailboxes (`\All`, `\Flagged`, `\Important`, such as Gmail's All Mail) are not mirrored, since
they repeat messages already mirrored elsewhere. When Files storage is full, mirroring stops
with a message and resumes when there is room.

## Reading and writing

- **Reading:** threads by References/In-Reply-To across mailboxes; HTML is sanitized on the
  server and again in the browser; remote images are blocked until Show images (or always,
  per Reading settings) and then load through Dar (`/api/v1/mail/image`) so senders do not see
  the reader's address; inline `cid:` images and attachments come from the `.eml`; attachments
  always download, never render as active content.
- **Actions:** read/unread, flag, archive, delete, move, spam, Show in Files, download `.eml`,
  multi-select with the tonal bar, Undo for archive/delete/move.
- **Writing:** plain text by default. Using the formatting bar (the Notes toolbar) makes the
  message Markdown and adds Write/Preview; it is then sent as
  `multipart/alternative`: the source as the text part and HTML that Dar renders from it
  (headings, lists and task lists, quotes, code, bold, italic, http/https/mailto links; any HTML
  typed is kept as text). A Markdown draft reopens as its source. Messages carry attachments (25 MB in total), To/Cc/Bcc chips (Bcc
  stays in the envelope), reply/reply all/forward with quoting and References; drafts are
  saved to the account's Drafts mailbox; every send waits ten seconds with Undo. SMTP accounts
  append a copy to Sent unless the provider says the server does it (`saveSent`); JMAP submits
  with the matching identity.
- **Search and keys:** subjects and senders appear in the shared search. `c` compose, `j`/`k`
  next/previous, `r`/`a`/`f` reply/reply all/forward, `e` archive, `#` delete, `s` flag,
  `u` unread, Ctrl+Enter send, Escape closes.

## Host configuration (NixOS)

```nix
services.dar.mail.providers = {
  # Google: register an OAuth client (type Web) with this redirect URL:
  #   https://cloud.example.com/api/v1/mail/oauth/callback
  google = {
    preset = "google"; name = "Google";
    oauth.clientId = "1234.apps.googleusercontent.com";
    oauth.clientSecretFile = "/run/secrets/dar-google-mail";   # never a store path
  };
  microsoft = {
    preset = "microsoft"; name = "Microsoft 365";
    oauth.clientId = "00000000-0000-0000-0000-000000000000";
    oauth.clientSecretFile = "/run/secrets/dar-microsoft-mail";
  };
  fastmail = { preset = "fastmail"; name = "Fastmail"; };       # JMAP with API tokens
  work = {
    name = "Work"; domains = [ "example.com" ];
    imap = { host = "mail.example.com"; port = 993; };
    smtp = { host = "mail.example.com"; port = 587; security = "starttls"; };
  };
};
```

Each provider takes `name`, `protocol` (`imap`, `jmap`), `auth` (`password`, `token`,
`oauth`), `domains` (suggest the provider for these addresses), `saveSent`, `imap` and
`smtp` (`host`, `port`, `security` = `tls` or `starttls`), `jmapUrl`, and `oauth`
(`authorizeUrl`, `tokenUrl`, `clientId`, `clientSecretFile`, `scopes`, `params`). Presets fill
in servers and endpoints. Client secrets reach the service as systemd credentials. The
generated file is `DAR_MAIL_PROVIDERS_FILE` (a JSON list of `{id, name, document,
secretFile?}`) for other hosts. Providers removed from the configuration are turned off, and
their accounts say so; providers added in Administration stay until removed there (only when
no account uses them).

## Limits

No offline editing or end-to-end encryption (S/MIME, PGP), no rich-text (WYSIWYG) composing, no IMAP
IDLE or JMAP push (mail arrives with the five-minute pass or Sync now), no rules or
vacation replies, no Contacts autocomplete yet. Backups carry the `.eml` files (Files), not
the accounts; after a restore, add the accounts again (their new folders get a free name).
