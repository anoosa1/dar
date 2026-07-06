# Email Client — mbsync + Maildir++ + SMTP Plan

> **For Hermes:** Execute tasks sequentially, verify each before committing.

**Goal:** Rewrite the email daemon to use `mbsync` (isync) for IMAP→Maildir++ sync, `net/smtp` for sending, and a Nextcloud Mail-compatible web UI.

**Architecture:** Daemon manages accounts, generates per-account `mbsync` configs, shells out to `mbsync` for sync, serves the resulting Maildir++ via REST API. Passwords encrypted with GPG via `pass`.

---

## Task 1: Account storage + mbsync config generation

**Objective:** Store accounts, generate `~/.mbsyncrc` entries on create/update.

**Files:**
- Rewrite: `cmd/email/main.go`

**Account struct:**
```go
type Account struct {
    ID        string `json:"id"`
    Email     string `json:"email"`
    Name      string `json:"name"`
    ImapHost  string `json:"imap_host"`
    ImapPort  int    `json:"imap_port"`
    ImapTLS   bool   `json:"imap_tls"`
    SmtpHost  string `json:"smtp_host"`
    SmtpPort  int    `json:"smtp_port"`
    SmtpTLS   bool   `json:"smtp_tls"`
    Username  string `json:"username"`
    Password  string `json:"-"` // loaded from pass on demand
}
```

**mbsync config generation** per account (written to a temp config or a single managed config):
```
IMAPAccount <account_id>
Host <imap_host>
Port <imap_port>
User <username>
PassCmd "pass dar/email/<account_id>"
SSLType IMAPS
CertificateFile /etc/ssl/certs/ca-certificates.crt

IMAPStore <account_id>-remote
Account <account_id>

MaildirStore <account_id>-local
Path <maildir_root>/
Inbox <maildir_root>/INBOX
SubFolders Verbatim

Channel <account_id>
Master :<account_id>-remote:
Slave :<account_id>-local:
Patterns *
Create Slave
SyncState *
```

**Verification:** Build, create account via API, verify `mbsync` config is generated correctly.

---

## Task 2: Account CRUD + mbsync config management

**Objective:** REST endpoints for accounts, regenerating mbsync config on changes.

**Endpoints:**
```
GET    /accounts              → list accounts (no passwords)
POST   /accounts              → create account + write mbsync config
PUT    /accounts?id=          → update + regenerate config  
DELETE /accounts?id=          → delete account + maildir + config
```

**Verification:** Create account → check config file exists. Delete → config + maildir gone.

---

## Task 3: mbsync shell-out for IMAP sync

**Objective:** Run `mbsync <account_id>` as a subprocess to sync emails.

**Implementation:**
```go
func syncAccount(accountID string) error {
    cmd := exec.Command("mbsync", accountID)
    cmd.Stdout = os.Stdout
    cmd.Stderr = os.Stderr
    return cmd.Run()
}
```

**Endpoints:**
```
POST /sync?account=    → trigger sync (blocking or background)
POST /sync-all          → sync all accounts
GET  /sync/status?id=   → check if sync is running
```

**Verification:** Create real IMAP account, POST sync, verify Maildir++ populated.

---

## Task 4: Maildir++ folder + message REST API

**Objective:** Serve emails from Maildir++ storage via REST API.

**Maildir++ layout per account:**
```
data/<user>/email/accounts/<id>/mail/
  INBOX/{cur,new,tmp}/
  .Sent/{cur,new,tmp}/
  .Drafts/{cur,new,tmp}/
  .Trash/{cur,new,tmp}/
```

**Message format:** Each file is raw RFC822. Parse headers for metadata. Body extracted for preview.

**Endpoints:**
```
GET  /folders?account=           → list folders + unread counts (count files in new/)
GET  /messages?folder=&account=&page=  → message list (from, subject, date, flags, size)
GET  /message?id=&account=&folder=     → full message (headers + body as text)
POST /message/move?id=&account=&folder= → move file between Maildir++ dirs
POST /message/flag?id=&account=&flag=  → set flags (seen, flagged, answered)
GET  /message/raw?id=&account=&folder= → raw RFC822 for download
```

**Verification:** Build, sync an account, verify API returns messages with correct counts and content.

---

## Task 5: SMTP send via `net/smtp`

**Objective:** Send emails using SMTP from account config.

**Implementation:**
- `POST /send` accepts: account_id, to, cc, bcc, subject, body
- Load account, connect to SMTP host:port with TLS if configured
- Use `smtp.SendMail` with `smtp.PlainAuth`
- After send, save copy to local Sent folder Maildir++ as RFC822

**Verification:** Send a test email through real SMTP, verify arrival, check Sent folder.

---

## Task 6: Nextcloud Mail web UI

**Objective:** Three-panel Nextcloud-style email interface using existing Material 3 colors.

**Files:**
- Rewrite: `internal/apps/email/handlers.go`

**Layout:**
- **Left sidebar** (w-56): Account switcher dropdown + folder list with unread badges
- **Middle panel** (flex-1, max-w-sm): Message list with sender, subject, preview, date, flags
- **Right panel** (flex-1): Full message view (headers + body)

**Features:**
- Compose modal (To, Cc, Bcc, Subject, body textarea)
- Reply / Reply All / Forward buttons in message view
- Refresh/sync button per account
- Search bar filtering local messages
- Account settings dialog (add/edit/delete)
- Read/unread toggle, star toggle, move to folder, delete
- Keyboard: `c` compose, `r` reply, `f` forward, `j/k` navigate

**Verification:** Load `/email`, navigate folders, view messages, compose + send.

---

## Task 7: Background auto-sync

**Objective:** Periodically sync all accounts via `mbsync` in background goroutine.

**Implementation:**
- Configurable interval (env `DAR_EMAIL_SYNC_MIN`, default 5)
- Per-account goroutine or sequential loop
- Skip sync if already running for that account
- Log sync results

**Verification:** Add account, wait for interval, check new messages appear.

---

## Task 8: E2E test

**Objective:** Full end-to-end test against real IMAP/SMTP.

**File:** `/tmp/hermes-verify-email-full.sh`

**Steps:**
1. Start email daemon
2. Create account from env vars
3. Trigger mbsync → verify folders + messages
4. Send test email via SMTP → verify in Sent
5. Check web UI pages load

**Verification:** All steps pass (or skip gracefully if no credentials).

---

## Dependencies

- `mbsync` (isync) — must be installed on the system (`nix-shell -p isync`)
- `net/smtp` — Go stdlib, no extra dep
- Already added `go-imap` can be removed from go.mod (not needed)

## Open questions

1. **pass/GPG setup:** Should I set up `pass` integration now or leave the `PassCmd` in mbsync config pointing to it for you to configure later?

2. **mbsync binary path:** Assume `mbsync` is on PATH, or configurable via env `DAR_MBSYNC_PATH`?

3. **Concurrent syncs:** OK to run one mbsync per account concurrently, or serialize all syncs?
