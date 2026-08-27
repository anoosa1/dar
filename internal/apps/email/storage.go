package email

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"net/mail"
	"strings"

	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS email_accounts (
		id TEXT PRIMARY KEY, user_id TEXT NOT NULL, email TEXT NOT NULL, name TEXT NOT NULL DEFAULT '', username TEXT NOT NULL,
		imap_host TEXT NOT NULL, imap_port INTEGER NOT NULL, imap_security TEXT NOT NULL,
		smtp_host TEXT NOT NULL, smtp_port INTEGER NOT NULL, smtp_security TEXT NOT NULL,
		password_key_id TEXT NOT NULL, password_nonce BLOB NOT NULL, password_cipher BLOB NOT NULL,
		enabled INTEGER NOT NULL DEFAULT 1, sync_minutes INTEGER NOT NULL DEFAULT 5, max_messages INTEGER NOT NULL DEFAULT 0,
		save_sent INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
		last_sync_at INTEGER NOT NULL DEFAULT 0, last_sync_error TEXT NOT NULL DEFAULT '', UNIQUE(user_id,email)
	);`,
	`CREATE INDEX IF NOT EXISTS idx_email_accounts_user ON email_accounts(user_id, enabled, email);`,
	`CREATE TABLE IF NOT EXISTS email_folders (
		id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL, name TEXT NOT NULL, role TEXT NOT NULL DEFAULT '',
		total_count INTEGER NOT NULL DEFAULT 0, unread_count INTEGER NOT NULL DEFAULT 0,
		UNIQUE(account_id,name), FOREIGN KEY(account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
	);`,
	`CREATE TABLE IF NOT EXISTS email_messages (
		id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL, content_hash TEXT NOT NULL, header_message_id TEXT NOT NULL DEFAULT '',
		thread_key TEXT NOT NULL, sender TEXT NOT NULL DEFAULT '', recipients TEXT NOT NULL DEFAULT '', cc TEXT NOT NULL DEFAULT '',
		bcc TEXT NOT NULL DEFAULT '', subject TEXT NOT NULL DEFAULT '', sent_at INTEGER NOT NULL DEFAULT 0, snippet TEXT NOT NULL DEFAULT '', text_body TEXT NOT NULL DEFAULT '',
		html_body TEXT NOT NULL DEFAULT '', size INTEGER NOT NULL DEFAULT 0, has_attachments INTEGER NOT NULL DEFAULT 0,
		parse_error TEXT NOT NULL DEFAULT '', UNIQUE(account_id,content_hash), FOREIGN KEY(account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_email_messages_thread ON email_messages(account_id,thread_key,sent_at DESC);`,
	`CREATE TABLE IF NOT EXISTS email_message_locations (
		id INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER NOT NULL, folder_id INTEGER NOT NULL, maildir_path TEXT NOT NULL,
		file_size INTEGER NOT NULL, file_mtime INTEGER NOT NULL, is_read INTEGER NOT NULL DEFAULT 0, is_flagged INTEGER NOT NULL DEFAULT 0,
		is_answered INTEGER NOT NULL DEFAULT 0, is_draft INTEGER NOT NULL DEFAULT 0, is_deleted INTEGER NOT NULL DEFAULT 0,
		UNIQUE(folder_id,maildir_path), FOREIGN KEY(message_id) REFERENCES email_messages(id) ON DELETE CASCADE,
		FOREIGN KEY(folder_id) REFERENCES email_folders(id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_email_locations_folder ON email_message_locations(folder_id,is_read,is_flagged,message_id);`,
	`CREATE TABLE IF NOT EXISTS email_recipients (
		message_id INTEGER NOT NULL, kind TEXT NOT NULL, ordinal INTEGER NOT NULL, name TEXT NOT NULL DEFAULT '', email TEXT NOT NULL,
		PRIMARY KEY(message_id,kind,ordinal), FOREIGN KEY(message_id) REFERENCES email_messages(id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_email_recipients_email ON email_recipients(email,message_id);`,
	`CREATE TABLE IF NOT EXISTS email_attachments (
		message_id INTEGER NOT NULL, part TEXT NOT NULL, filename TEXT NOT NULL DEFAULT '', mime_type TEXT NOT NULL,
		size INTEGER NOT NULL DEFAULT 0, content_id TEXT NOT NULL DEFAULT '', is_inline INTEGER NOT NULL DEFAULT 0,
		PRIMARY KEY(message_id,part), FOREIGN KEY(message_id) REFERENCES email_messages(id) ON DELETE CASCADE
	);`,
	`CREATE TABLE IF NOT EXISTS email_sync_runs (
		id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL, trigger_name TEXT NOT NULL, state TEXT NOT NULL,
		started_at INTEGER NOT NULL DEFAULT 0, ended_at INTEGER NOT NULL DEFAULT 0, scanned INTEGER NOT NULL DEFAULT 0,
		updated INTEGER NOT NULL DEFAULT 0, removed INTEGER NOT NULL DEFAULT 0, error_text TEXT NOT NULL DEFAULT '',
		FOREIGN KEY(account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
	);`,
	`CREATE TABLE IF NOT EXISTS email_operations (
		id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
		phase TEXT NOT NULL, state TEXT NOT NULL, error_text TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
		FOREIGN KEY(account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
	);`,
	`CREATE VIRTUAL TABLE IF NOT EXISTS email_fts USING fts5(message_id UNINDEXED,user_id UNINDEXED,subject,participants,body,filenames,tokenize='unicode61 remove_diacritics 2');`,
}

func Migrations() []string { return migrations }

var errNotFound = errors.New("not found")

func newID() string { var b [16]byte; _, _ = rand.Read(b[:]); return hex.EncodeToString(b[:]) }
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}
func blob(s *sqlite.Stmt, col int) []byte {
	b := make([]byte, s.ColumnLen(col))
	s.ColumnBytes(col, b)
	return b
}

func (a *App) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return errors.New("email database unavailable")
	}
	c, release, err := a.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}

func scanAccount(s *sqlite.Stmt) Account {
	return Account{ID: s.ColumnText(0), Email: s.ColumnText(1), Name: s.ColumnText(2), Username: s.ColumnText(3), IMAPHost: s.ColumnText(4), IMAPPort: int(s.ColumnInt64(5)), IMAPSecurity: securityMode(s.ColumnText(6)), SMTPHost: s.ColumnText(7), SMTPPort: int(s.ColumnInt64(8)), SMTPSecurity: securityMode(s.ColumnText(9)), PasswordKeyID: s.ColumnText(10), PasswordNonce: blob(s, 11), PasswordCipher: blob(s, 12), Enabled: s.ColumnInt64(13) != 0, SyncMinutes: int(s.ColumnInt64(14)), MaxMessages: int(s.ColumnInt64(15)), SaveSent: s.ColumnInt64(16) != 0, LastSyncAt: s.ColumnInt64(17), LastSyncError: s.ColumnText(18)}
}

const accountColumns = `id,email,name,username,imap_host,imap_port,imap_security,smtp_host,smtp_port,smtp_security,password_key_id,password_nonce,password_cipher,enabled,sync_minutes,max_messages,save_sent,last_sync_at,last_sync_error`

func (a *App) accounts(ctx context.Context, uid string) ([]Account, error) {
	var out []Account
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT `+accountColumns+` FROM email_accounts WHERE user_id=? ORDER BY lower(email)`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { out = append(out, scanAccount(s)); return nil }})
	})
	if out == nil {
		out = []Account{}
	}
	return out, err
}

func (a *App) account(ctx context.Context, uid, id string) (Account, error) {
	var out Account
	found := false
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT `+accountColumns+` FROM email_accounts WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error { out = scanAccount(s); found = true; return nil }})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return out, err
}

func normalizeAccount(in *Account) error {
	in.Email = strings.TrimSpace(strings.ToLower(in.Email))
	in.Username = strings.TrimSpace(in.Username)
	in.IMAPHost = strings.TrimSpace(in.IMAPHost)
	in.SMTPHost = strings.TrimSpace(in.SMTPHost)
	in.Name = strings.TrimSpace(in.Name)
	if in.Email == "" || !strings.Contains(in.Email, "@") || in.Username == "" || in.IMAPHost == "" || in.SMTPHost == "" {
		return errors.New("email, username, IMAP host and SMTP host are required")
	}
	parsed, err := mail.ParseAddress(in.Email)
	if err != nil || !strings.EqualFold(parsed.Address, in.Email) || len(in.Email) > 254 || in.Email == "." || in.Email == ".." || strings.ContainsAny(in.Email, `/\`) {
		return errors.New("email must be a safe normalized mailbox address")
	}
	for _, r := range in.Email {
		if r < 0x20 || r == 0x7f {
			return errors.New("email contains an unsafe path character")
		}
	}
	if in.IMAPPort == 0 {
		in.IMAPPort = 993
	}
	if in.SMTPPort == 0 {
		in.SMTPPort = 587
	}
	if in.IMAPSecurity == "" {
		in.IMAPSecurity = securityTLS
	}
	if in.SMTPSecurity == "" {
		in.SMTPSecurity = securitySTARTTLS
	}
	valid := func(v securityMode) bool { return v == securityTLS || v == securitySTARTTLS || v == securityNone }
	if !valid(in.IMAPSecurity) || !valid(in.SMTPSecurity) {
		return errors.New("invalid transport security mode")
	}
	if (in.IMAPSecurity == securityNone && !isLoopbackHost(in.IMAPHost)) || (in.SMTPSecurity == securityNone && !isLoopbackHost(in.SMTPHost)) {
		return errors.New("plaintext mail transport is allowed only for loopback hosts")
	}
	if in.SyncMinutes <= 0 {
		in.SyncMinutes = 5
	}
	if in.MaxMessages < 0 {
		return errors.New("max_messages cannot be negative")
	}
	return nil
}

func (a *App) createAccount(ctx context.Context, uid string, in Account) (Account, error) {
	if in.SyncMinutes <= 0 {
		in.SyncMinutes = a.syncMinutes
	}
	if err := normalizeAccount(&in); err != nil {
		return Account{}, err
	}
	if in.Password == "" {
		return Account{}, errors.New("password or app password is required")
	}
	if in.ID == "" {
		in.ID = newID()
	}
	keyID, nonce, ciphertext, err := a.keys.encrypt(uid, in.ID, in.Password)
	if err != nil {
		return Account{}, err
	}
	now := nowUnix()
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_accounts(id,user_id,email,name,username,imap_host,imap_port,imap_security,smtp_host,smtp_port,smtp_security,password_key_id,password_nonce,password_cipher,enabled,sync_minutes,max_messages,save_sent,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{in.ID, uid, in.Email, in.Name, in.Username, in.IMAPHost, in.IMAPPort, string(in.IMAPSecurity), in.SMTPHost, in.SMTPPort, string(in.SMTPSecurity), keyID, nonce, ciphertext, boolInt(in.Enabled), in.SyncMinutes, in.MaxMessages, boolInt(in.SaveSent), now, now}})
	})
	if err != nil {
		return Account{}, err
	}
	if _, err := a.ensureManagedMailRoot(uid, in); err != nil {
		_ = a.withConn(ctx, func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM email_accounts WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, in.ID}})
		})
		return Account{}, fmt.Errorf("create Maildir: %w", err)
	}
	in.Password = ""
	return a.account(ctx, uid, in.ID)
}

func (a *App) updateAccount(ctx context.Context, uid, id string, in Account) (Account, error) {
	lock := a.lockFor(id)
	lock.Lock()
	defer lock.Unlock()
	old, err := a.account(ctx, uid, id)
	if err != nil {
		return Account{}, err
	}
	in.ID = id
	if in.SyncMinutes <= 0 {
		in.SyncMinutes = a.syncMinutes
	}
	if in.Password == "" {
		in.PasswordKeyID = old.PasswordKeyID
		in.PasswordNonce = old.PasswordNonce
		in.PasswordCipher = old.PasswordCipher
	} else {
		in.PasswordKeyID, in.PasswordNonce, in.PasswordCipher, err = a.keys.encrypt(uid, id, in.Password)
		if err != nil {
			return Account{}, err
		}
	}
	if err := normalizeAccount(&in); err != nil {
		return Account{}, err
	}
	if _, err := a.ensureManagedMailRoot(uid, old); err != nil {
		return Account{}, fmt.Errorf("prepare existing Maildir: %w", err)
	}
	mailMoved := !strings.EqualFold(old.Email, in.Email)
	if _, err := a.ensureManagedMailRoot(uid, in); err != nil {
		return Account{}, fmt.Errorf("move Maildir: %w", err)
	}
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE email_accounts SET email=?,name=?,username=?,imap_host=?,imap_port=?,imap_security=?,smtp_host=?,smtp_port=?,smtp_security=?,password_key_id=?,password_nonce=?,password_cipher=?,enabled=?,sync_minutes=?,max_messages=?,save_sent=?,updated_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{in.Email, in.Name, in.Username, in.IMAPHost, in.IMAPPort, string(in.IMAPSecurity), in.SMTPHost, in.SMTPPort, string(in.SMTPSecurity), in.PasswordKeyID, in.PasswordNonce, in.PasswordCipher, boolInt(in.Enabled), in.SyncMinutes, in.MaxMessages, boolInt(in.SaveSent), nowUnix(), uid, id}})
	})
	if err != nil {
		if mailMoved {
			_, _ = a.ensureManagedMailRoot(uid, old)
		}
		return Account{}, err
	}
	return a.account(ctx, uid, id)
}

func (a *App) deleteAccount(ctx context.Context, uid, id string) error {
	if _, err := a.account(ctx, uid, id); err != nil {
		return err
	}
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM email_accounts WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
	})
	if err == nil {
		a.mu.Lock()
		delete(a.accountLocks, id)
		delete(a.active, uid+"/"+id)
		delete(a.status, uid+"/"+id)
		a.mu.Unlock()
	}
	return err
}

func (a *App) password(ctx context.Context, uid string, acct Account) (string, error) {
	plain, rotate, err := a.keys.decrypt(uid, acct.ID, acct.PasswordKeyID, acct.PasswordNonce, acct.PasswordCipher)
	if err != nil {
		return "", err
	}
	if rotate {
		kid, n, cipherData, e := a.keys.encrypt(uid, acct.ID, plain)
		if e == nil {
			_ = a.withConn(ctx, func(conn *sqlite.Conn) error {
				return sqlitex.Execute(conn, `UPDATE email_accounts SET password_key_id=?,password_nonce=?,password_cipher=?,updated_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{kid, n, cipherData, nowUnix(), uid, acct.ID}})
			})
		}
	}
	return plain, nil
}

type accountOwner struct{ UserID, AccountID string }

func (a *App) validateStoredCredentials(ctx context.Context) error {
	var owners []accountOwner
	if err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT user_id,id FROM email_accounts`, &sqlitex.ExecOptions{ResultFunc: func(s *sqlite.Stmt) error {
			owners = append(owners, accountOwner{UserID: s.ColumnText(0), AccountID: s.ColumnText(1)})
			return nil
		}})
	}); err != nil {
		return err
	}
	for _, owner := range owners {
		acct, err := a.account(ctx, owner.UserID, owner.AccountID)
		if err != nil {
			return err
		}
		if _, err := a.password(ctx, owner.UserID, acct); err != nil {
			return errors.New("stored Email credentials cannot be decrypted with the configured keys")
		}
		if _, err := a.ensureManagedMailRoot(owner.UserID, acct); err != nil {
			return fmt.Errorf("relocate Maildir for %s: %w", acct.Email, err)
		}
	}
	return nil
}

func (a *App) dueAccountIDs(ctx context.Context, now int64) ([]accountOwner, error) {
	var out []accountOwner
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT user_id,id FROM email_accounts WHERE enabled=1 AND (last_sync_at=0 OR last_sync_at+(sync_minutes*60)<=?)`, &sqlitex.ExecOptions{Args: []any{now}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, accountOwner{s.ColumnText(0), s.ColumnText(1)})
			return nil
		}})
	})
	return out, err
}

func (a *App) folders(ctx context.Context, uid, accountID string) ([]Folder, error) {
	var out []Folder
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT f.id,f.account_id,f.name,f.role,f.total_count,f.unread_count FROM email_folders f JOIN email_accounts a ON a.id=f.account_id WHERE a.user_id=? AND (?='' OR f.account_id=?) ORDER BY CASE f.role WHEN 'inbox' THEN 0 WHEN 'starred' THEN 1 WHEN 'drafts' THEN 2 WHEN 'sent' THEN 3 WHEN 'archive' THEN 4 WHEN 'junk' THEN 5 WHEN 'trash' THEN 6 ELSE 7 END,lower(f.name)`, &sqlitex.ExecOptions{Args: []any{uid, accountID, accountID}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Folder{ID: s.ColumnInt64(0), AccountID: s.ColumnText(1), Name: s.ColumnText(2), Role: s.ColumnText(3), Total: int(s.ColumnInt64(4)), Unread: int(s.ColumnInt64(5))})
			return nil
		}})
	})
	if out == nil {
		out = []Folder{}
	}
	return out, err
}

func (a *App) preferences(ctx context.Context, uid string) (Preferences, error) {
	p := defaultPreferences()
	err := a.withConn(ctx, func(c *sqlite.Conn) error { _, err := preferences.GetJSON(c, uid, "email", "settings", &p); return err })
	return p, err
}
func (a *App) savePreferences(ctx context.Context, uid string, p Preferences) error {
	if p.Density != "compact" && p.Density != "comfortable" {
		return errors.New("invalid density")
	}
	if p.ReadingPane != "vertical" && p.ReadingPane != "horizontal" && p.ReadingPane != "off" {
		return errors.New("invalid reading pane")
	}
	return a.withConn(ctx, func(c *sqlite.Conn) error { return preferences.SetJSON(c, uid, "email", "settings", p) })
}

func (a *App) recordSync(ctx context.Context, st SyncStatus) error {
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_sync_runs(account_id,trigger_name,state,started_at,ended_at,scanned,updated,removed,error_text) VALUES(?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{st.AccountID, st.Trigger, st.State, st.StartedAt, st.EndedAt, st.Scanned, st.Updated, st.Removed, st.Error}})
	})
}
func (a *App) setAccountSyncResult(ctx context.Context, uid, accountID, errText string) error {
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE email_accounts SET last_sync_at=?,last_sync_error=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{nowUnix(), errText, uid, accountID}})
	})
}

func ownedMessageWhere() string {
	return ` FROM email_messages m JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND m.id=?`
}
func (a *App) messageOwner(ctx context.Context, uid string, id int64) (string, error) {
	var accountID string
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT m.account_id`+ownedMessageWhere(), &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error { accountID = s.ColumnText(0); return nil }})
	})
	if err == nil && accountID == "" {
		err = errNotFound
	}
	return accountID, err
}

func sqlPlaceholders(n int) string {
	if n <= 0 {
		return ""
	}
	return strings.TrimRight(strings.Repeat("?,", n), ",")
}
func wrapDBErr(op string, err error) error {
	if err == nil {
		return nil
	}
	return fmt.Errorf("%s: %w", op, err)
}
