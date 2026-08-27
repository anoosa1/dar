package email

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func mbsyncTLS(v securityMode) string {
	switch v {
	case securityTLS:
		return "IMAPS"
	case securitySTARTTLS:
		return "STARTTLS"
	default:
		return "None"
	}
}

func (a *App) writeMBSyncConfig(uid string, acct Account) (string, error) {
	root, err := a.ensureManagedMailRoot(uid, acct)
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Join(root, "INBOX", "cur"), 0700); err != nil {
		return "", err
	}
	for _, sub := range []string{"new", "tmp"} {
		_ = os.MkdirAll(filepath.Join(root, "INBOX", sub), 0700)
	}
	q := strconv.Quote
	tunnel := shellQuote(a.tunnelPath) + " --mail-tunnel-host " + shellQuote(acct.IMAPHost) + " --mail-tunnel-port " + strconv.Itoa(acct.IMAPPort)
	cfg := fmt.Sprintf(`IMAPAccount %s
Host %s
Port %d
Tunnel %s
User %s
PassCmd "printenv DAR_EMAIL_ACCOUNT_PASSWORD"
TLSType %s
SystemCertificates yes

IMAPStore %s-far
Account %s

MaildirStore %s-near
Path %s
Inbox %s
SubFolders Verbatim

Channel %s
Far :%s-far:
Near :%s-near:
Patterns INBOX *
Create Both
Remove Both
Sync Full
CopyArrivalDate yes
`, acct.ID, q(acct.IMAPHost), acct.IMAPPort, q(tunnel), q(acct.Username), mbsyncTLS(acct.IMAPSecurity), acct.ID, acct.ID, acct.ID, q(root+string(filepath.Separator)), q(filepath.Join(root, "INBOX")), acct.ID, acct.ID, acct.ID)
	if acct.MaxMessages > 0 {
		cfg += fmt.Sprintf("MaxMessages %d\nExpireUnread no\nExpireSide Near\n", acct.MaxMessages)
	}
	path := filepath.Join(a.accountRoot(uid, acct.ID), "mbsyncrc")
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return "", err
	}
	if err := os.WriteFile(path, []byte(cfg), 0600); err != nil {
		return "", err
	}
	return path, nil
}

func (a *App) runMBSync(ctx context.Context, uid string, acct Account, listOnly, expunge bool) (string, error) {
	password, err := a.password(ctx, uid, acct)
	if err != nil {
		return "", err
	}
	cfg, err := a.writeMBSyncConfig(uid, acct)
	if err != nil {
		return "", err
	}
	args := []string{"-c", cfg}
	if listOnly {
		args = append(args, "-l")
	} else if expunge {
		args = append(args, "--expunge-far")
	}
	args = append(args, acct.ID)
	cmd := exec.CommandContext(ctx, a.mbsyncPath, args...)
	cmd.Env = []string{
		"PATH=" + os.Getenv("PATH"),
		"HOME=" + a.accountRoot(uid, acct.ID),
		"LANG=C.UTF-8",
		"LC_ALL=C.UTF-8",
		"DAR_EMAIL_ACCOUNT_PASSWORD=" + password,
		"DAR_OUTBOUND_ALLOWED_HOSTS=" + os.Getenv("DAR_OUTBOUND_ALLOWED_HOSTS"),
		"DAR_OUTBOUND_ALLOWED_CIDRS=" + os.Getenv("DAR_OUTBOUND_ALLOWED_CIDRS"),
	}
	var output limitedBuffer
	cmd.Stdout = &output
	cmd.Stderr = &output
	err = cmd.Run()
	if err != nil {
		return output.String(), fmt.Errorf("mbsync: %w", err)
	}
	return output.String(), nil
}

func shellQuote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", "'\"'\"'") + "'"
}

type limitedBuffer struct{ bytes.Buffer }

func (b *limitedBuffer) Write(p []byte) (int, error) {
	const max = 32 << 10
	if b.Len() >= max {
		return len(p), nil
	}
	if len(p) > max-b.Len() {
		p = p[:max-b.Len()]
	}
	return b.Buffer.Write(p)
}

func (a *App) testAccount(ctx context.Context, uid string, acct Account) (map[string]string, error) {
	if err := a.available(); err != nil {
		return nil, err
	}
	if err := normalizeAccount(&acct); err != nil {
		return nil, err
	}
	if acct.Password == "" {
		return nil, errors.New("password or app password is required")
	}
	acct.ID = "test-" + newID()
	acct.mailRootOverride = filepath.Join(a.accountRoot(uid, acct.ID), "mail")
	acct.PasswordKeyID, acct.PasswordNonce, acct.PasswordCipher = "", nil, nil
	defer os.RemoveAll(a.accountRoot(uid, acct.ID))
	kid, n, c, err := a.keys.encrypt(uid, acct.ID, acct.Password)
	if err != nil {
		return nil, err
	}
	acct.PasswordKeyID = kid
	acct.PasswordNonce = n
	acct.PasswordCipher = c
	out, imapErr := a.runMBSync(ctx, uid, acct, true, false)
	smtpErr := a.testSMTP(ctx, acct, acct.Password)
	result := map[string]string{"imap": "ok", "smtp": "ok"}
	if imapErr != nil {
		result["imap"] = truncateError(imapErr.Error() + " " + out)
	}
	if smtpErr != nil {
		result["smtp"] = truncateError(smtpErr.Error())
	}
	if imapErr != nil || smtpErr != nil {
		return result, errors.New("connection test failed")
	}
	return result, nil
}
func truncateError(s string) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > 500 {
		return s[:500]
	}
	return s
}

func (a *App) queueSync(uid, accountID, trigger string) bool {
	if a.available() != nil || a.ctx == nil {
		return false
	}
	key := uid + "/" + accountID
	a.mu.Lock()
	if a.active[key] {
		a.mu.Unlock()
		return false
	}
	a.active[key] = true
	a.status[key] = SyncStatus{AccountID: accountID, State: "queued", Trigger: trigger}
	a.mu.Unlock()
	a.wg.Add(1)
	go func() {
		defer a.wg.Done()
		select {
		case a.workers <- struct{}{}:
		case <-a.ctx.Done():
			a.finishActive(key)
			return
		}
		defer func() { <-a.workers }()
		a.syncAccount(uid, accountID, trigger)
		a.finishActive(key)
	}()
	return true
}
func (a *App) finishActive(key string) { a.mu.Lock(); delete(a.active, key); a.mu.Unlock() }
func (a *App) syncStatus(uid, accountID string) SyncStatus {
	a.mu.Lock()
	defer a.mu.Unlock()
	if st, ok := a.status[uid+"/"+accountID]; ok {
		return st
	}
	return SyncStatus{AccountID: accountID, State: "idle"}
}

func (a *App) syncAccount(uid, accountID, trigger string) {
	lock := a.lockFor(accountID)
	lock.Lock()
	defer lock.Unlock()
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	st := SyncStatus{AccountID: accountID, State: "running", Trigger: trigger, StartedAt: nowUnix()}
	key := uid + "/" + accountID
	a.mu.Lock()
	a.status[key] = st
	a.mu.Unlock()
	acct, err := a.account(ctx, uid, accountID)
	if err == nil && !acct.Enabled {
		err = errors.New("account is disabled")
	}
	expunge := trigger == "operation" || a.hasPendingOperations(ctx, accountID)
	if err == nil {
		_, err = a.runMBSync(ctx, uid, acct, false, expunge)
	}
	var stats indexStats
	if err == nil {
		stats, err = a.reindexAccount(ctx, uid, accountID)
	} else {
		stats, _ = a.reindexAccount(ctx, uid, accountID)
	}
	st.Scanned = stats.Scanned
	st.Updated = stats.Updated
	st.Removed = stats.Removed
	st.EndedAt = nowUnix()
	if err != nil {
		st.State = "failed"
		st.Error = truncateError(err.Error())
	} else {
		st.State = "succeeded"
		_ = a.completeOperations(ctx, accountID)
	}
	a.notifyNewMessages(ctx, uid, acct, stats.NewMessages)
	_ = a.setAccountSyncResult(ctx, uid, accountID, st.Error)
	_ = a.recordSync(ctx, st)
	summary := fmt.Sprintf("email sync %s: %d scanned, %d updated, %d removed", st.State, st.Scanned, st.Updated, st.Removed)
	a.recordActivity(ctx, uid, "mail_sync", summary, "sync", accountID, "Email sync", "background")
	a.mu.Lock()
	a.status[key] = st
	a.mu.Unlock()
}

func (a *App) notifyNewMessages(ctx context.Context, uid string, acct Account, messages []newIndexedMessage) {
	if a.registry == nil || acct.LastSyncAt <= 0 {
		return
	}
	for _, message := range messages {
		subject := strings.TrimSpace(message.Subject)
		if subject == "" {
			subject = "(no subject)"
		}
		heading := "New email"
		if sender := strings.TrimSpace(message.From); sender != "" {
			heading += " from " + sender
		}
		detail := subject
		if account := strings.TrimSpace(acct.Email); account != "" {
			detail += " · " + account
		}
		if folder := strings.TrimSpace(message.Folder); folder != "" {
			detail += " · " + folder
		}
		objectURL := "/email?account_id=" + url.QueryEscape(acct.ID) + "&thread=" + url.QueryEscape(message.ThreadKey)
		if err := a.registry.EnqueueNotification(ctx, apps.Notification{
			RecipientID: uid,
			Source:      "email",
			Kind:        "new_mail",
			Subject:     heading,
			Message:     detail,
			Icon:        "fa-solid fa-envelope",
			ObjectURL:   objectURL,
			DedupeKey:   acct.ID + ":" + message.ContentHash,
		}); err != nil {
			log.Printf("record new email notification: %v", err)
		}
	}
}

func (a *App) hasPendingOperations(ctx context.Context, accountID string) bool {
	found := false
	_ = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT 1 FROM email_operations WHERE account_id=? AND state='pending' LIMIT 1`, &sqlitex.ExecOptions{Args: []any{accountID}, ResultFunc: func(*sqlite.Stmt) error { found = true; return nil }})
	})
	return found
}

func (a *App) completeOperations(ctx context.Context, accountID string) error {
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE email_operations SET state='completed',phase='synced',updated_at=? WHERE account_id=? AND state='pending'`, &sqlitex.ExecOptions{Args: []any{nowUnix(), accountID}})
	})
}

func (a *App) waitSync(accountID string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		a.mu.Lock()
		active := false
		for key := range a.active {
			if strings.HasSuffix(key, "/"+accountID) {
				active = true
				break
			}
		}
		a.mu.Unlock()
		if !active {
			return nil
		}
		time.Sleep(20 * time.Millisecond)
	}
	return context.DeadlineExceeded
}
