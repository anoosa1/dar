package email

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func threadID(accountID, key string) string {
	return base64.RawURLEncoding.EncodeToString([]byte(accountID + "\x00" + key))
}
func parseThreadID(id string) (string, string, error) {
	b, err := base64.RawURLEncoding.DecodeString(id)
	if err != nil {
		return "", "", err
	}
	parts := strings.SplitN(string(b), "\x00", 2)
	if len(parts) != 2 {
		return "", "", errors.New("invalid thread id")
	}
	return parts[0], parts[1], nil
}
func encodeCursor(at int64, id string) string {
	return base64.RawURLEncoding.EncodeToString([]byte(strconv.FormatInt(at, 10) + "\x00" + id))
}
func decodeCursor(v string) (int64, string, error) {
	if v == "" {
		return 0, "", nil
	}
	b, err := base64.RawURLEncoding.DecodeString(v)
	if err != nil {
		return 0, "", err
	}
	p := strings.SplitN(string(b), "\x00", 2)
	if len(p) != 2 {
		return 0, "", errors.New("invalid cursor")
	}
	n, err := strconv.ParseInt(p[0], 10, 64)
	return n, p[1], err
}

type threadQuery struct {
	AccountID, Role                string
	FolderID                       int64
	Query                          string
	Unread, Flagged, HasAttachment bool
	Before, After                  int64
	Cursor                         string
	Limit                          int
}

func ftsQuery(q string) string {
	var terms []string
	for _, x := range strings.Fields(q) {
		x = strings.ReplaceAll(x, `"`, `""`)
		if x != "" {
			terms = append(terms, `"`+x+`"*`)
		}
	}
	return strings.Join(terms, " AND ")
}

func (a *App) threads(ctx context.Context, uid string, q threadQuery) ([]ThreadSummary, string, error) {
	if q.Limit <= 0 {
		q.Limit = 50
	}
	if q.Limit > 100 {
		q.Limit = 100
	}
	where := []string{"a.user_id=?"}
	args := []any{uid}
	if q.AccountID != "" {
		where = append(where, "m.account_id=?")
		args = append(args, q.AccountID)
	}
	if q.FolderID > 0 {
		where = append(where, "f.id=?")
		args = append(args, q.FolderID)
	} else if q.Role != "" && q.Role != "all" && q.Role != "starred" {
		where = append(where, "f.role=?")
		args = append(args, q.Role)
	}
	if q.Role == "starred" || q.Flagged {
		where = append(where, "l.is_flagged=1")
	}
	if q.Unread {
		where = append(where, "l.is_read=0")
	}
	if q.HasAttachment {
		where = append(where, "m.has_attachments=1")
	}
	if q.Before > 0 {
		where = append(where, "m.sent_at<?")
		args = append(args, q.Before)
	}
	if q.After > 0 {
		where = append(where, "m.sent_at>?")
		args = append(args, q.After)
	}
	joinFTS := ""
	if strings.TrimSpace(q.Query) != "" {
		where = append(where, "EXISTS (SELECT 1 FROM email_messages m2 JOIN email_fts ON CAST(email_fts.message_id AS INTEGER)=m2.id WHERE m2.account_id=m.account_id AND m2.thread_key=m.thread_key AND email_fts.user_id=? AND email_fts MATCH ?)")
		args = append(args, uid, ftsQuery(q.Query))
	}
	sql := `SELECT m.account_id,m.thread_key,a.email,COALESCE(NULLIF(max(m.subject),''),'(no subject)'),group_concat(DISTINCT m.sender),max(m.snippet),max(m.sent_at),count(DISTINCT m.id),sum(CASE WHEN l.is_read=0 THEN 1 ELSE 0 END),max(l.is_flagged),max(m.has_attachments) FROM email_messages m JOIN email_accounts a ON a.id=m.account_id JOIN email_message_locations l ON l.message_id=m.id JOIN email_folders f ON f.id=l.folder_id` + joinFTS + ` WHERE ` + strings.Join(where, " AND ") + ` GROUP BY m.account_id,m.thread_key ORDER BY max(m.sent_at) DESC,m.thread_key DESC LIMIT 500`
	var out []ThreadSummary
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, sql, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error {
			id := threadID(s.ColumnText(0), s.ColumnText(1))
			out = append(out, ThreadSummary{ID: id, AccountID: s.ColumnText(0), AccountEmail: s.ColumnText(2), Subject: s.ColumnText(3), Participants: s.ColumnText(4), Snippet: s.ColumnText(5), LatestAt: s.ColumnInt64(6), MessageCount: int(s.ColumnInt64(7)), UnreadCount: int(s.ColumnInt64(8)), Flagged: s.ColumnInt64(9) != 0, HasAttachments: s.ColumnInt64(10) != 0})
			return nil
		}})
	})
	if err != nil {
		return nil, "", err
	}
	cursorAt, cursorID, err := decodeCursor(q.Cursor)
	if err != nil {
		return nil, "", err
	}
	if cursorAt > 0 {
		filtered := out[:0]
		for _, x := range out {
			if x.LatestAt < cursorAt || (x.LatestAt == cursorAt && x.ID < cursorID) {
				filtered = append(filtered, x)
			}
		}
		out = filtered
	}
	next := ""
	if len(out) > q.Limit {
		out = out[:q.Limit]
		last := out[len(out)-1]
		next = encodeCursor(last.LatestAt, last.ID)
	}
	if out == nil {
		out = []ThreadSummary{}
	}
	return out, next, nil
}

func (a *App) threadMessages(ctx context.Context, uid, id string) ([]Message, error) {
	accountID, key, err := parseThreadID(id)
	if err != nil {
		return nil, err
	}
	if _, err := a.account(ctx, uid, accountID); err != nil {
		return nil, err
	}
	var out []Message
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT m.id,m.account_id,min(l.folder_id),COALESCE(min(f.role),''),m.thread_key,m.header_message_id,m.sender,m.recipients,m.cc,m.bcc,m.subject,m.sent_at,m.text_body,(m.html_body!=''),max(l.is_read),max(l.is_flagged),max(l.is_answered),max(l.is_draft) FROM email_messages m JOIN email_message_locations l ON l.message_id=m.id JOIN email_folders f ON f.id=l.folder_id WHERE m.account_id=? AND m.thread_key=? GROUP BY m.id ORDER BY m.sent_at,m.id`, &sqlitex.ExecOptions{Args: []any{accountID, key}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Message{ID: s.ColumnInt64(0), AccountID: s.ColumnText(1), FolderID: s.ColumnInt64(2), FolderRole: s.ColumnText(3), ThreadKey: s.ColumnText(4), HeaderMessageID: s.ColumnText(5), From: s.ColumnText(6), To: s.ColumnText(7), Cc: s.ColumnText(8), Bcc: s.ColumnText(9), Subject: s.ColumnText(10), Date: s.ColumnInt64(11), Text: s.ColumnText(12), HasHTML: s.ColumnInt64(13) != 0, Read: s.ColumnInt64(14) != 0, Flagged: s.ColumnInt64(15) != 0, Answered: s.ColumnInt64(16) != 0, Draft: s.ColumnInt64(17) != 0})
			return nil
		}})
	})
	if err != nil {
		return nil, err
	}
	if len(out) == 0 {
		return nil, errNotFound
	}
	for i := range out {
		out[i].Attachments, _ = a.attachments(ctx, uid, out[i].ID)
	}
	return out, nil
}

func (a *App) attachments(ctx context.Context, uid string, messageID int64) ([]Attachment, error) {
	var out []Attachment
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT x.part,x.filename,x.mime_type,x.size,x.content_id,x.is_inline FROM email_attachments x JOIN email_messages m ON m.id=x.message_id JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND m.id=? ORDER BY x.part`, &sqlitex.ExecOptions{Args: []any{uid, messageID}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Attachment{Part: s.ColumnText(0), Filename: s.ColumnText(1), MIMEType: s.ColumnText(2), Size: s.ColumnInt64(3), ContentID: s.ColumnText(4), Inline: s.ColumnInt64(5) != 0})
			return nil
		}})
	})
	if out == nil {
		out = []Attachment{}
	}
	return out, err
}

func (a *App) messagePath(ctx context.Context, uid string, messageID int64) (string, Account, error) {
	var rel string
	var acct Account
	found := false
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT l.maildir_path,a.id,a.email,a.name,a.username,a.imap_host,a.imap_port,a.imap_security,a.smtp_host,a.smtp_port,a.smtp_security,a.password_key_id,a.password_nonce,a.password_cipher,a.enabled,a.sync_minutes,a.max_messages,a.save_sent,a.last_sync_at,a.last_sync_error FROM email_message_locations l JOIN email_messages m ON m.id=l.message_id JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND m.id=? ORDER BY l.id LIMIT 1`, &sqlitex.ExecOptions{Args: []any{uid, messageID}, ResultFunc: func(s *sqlite.Stmt) error {
			rel = s.ColumnText(0)
			acct = Account{ID: s.ColumnText(1), Email: s.ColumnText(2), Name: s.ColumnText(3), Username: s.ColumnText(4), IMAPHost: s.ColumnText(5), IMAPPort: int(s.ColumnInt64(6)), IMAPSecurity: securityMode(s.ColumnText(7)), SMTPHost: s.ColumnText(8), SMTPPort: int(s.ColumnInt64(9)), SMTPSecurity: securityMode(s.ColumnText(10)), PasswordKeyID: s.ColumnText(11), PasswordNonce: blob(s, 12), PasswordCipher: blob(s, 13), Enabled: s.ColumnInt64(14) != 0, SyncMinutes: int(s.ColumnInt64(15)), MaxMessages: int(s.ColumnInt64(16)), SaveSent: s.ColumnInt64(17) != 0, LastSyncAt: s.ColumnInt64(18), LastSyncError: s.ColumnText(19)}
			found = true
			return nil
		}})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return filepathJoin(a.mailRoot(uid, acct.ID), rel), acct, err
}

func filepathJoin(root, rel string) string {
	rel = strings.TrimPrefix(rel, "/")
	return root + "/" + rel
}

func (a *App) recentRecipients(ctx context.Context, uid, q string, limit int) ([]struct{ Name, Email string }, error) {
	if limit <= 0 || limit > 50 {
		limit = 10
	}
	var out []struct{ Name, Email string }
	like := "%" + strings.ToLower(strings.TrimSpace(q)) + "%"
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT r.name,r.email,max(m.sent_at) recent,count(*) freq FROM email_recipients r JOIN email_messages m ON m.id=r.message_id JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND r.kind IN ('to','cc') AND (lower(r.email) LIKE ? OR lower(r.name) LIKE ?) GROUP BY lower(r.email) ORDER BY recent DESC,freq DESC LIMIT ?`, &sqlitex.ExecOptions{Args: []any{uid, like, like, limit}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, struct{ Name, Email string }{s.ColumnText(0), s.ColumnText(1)})
			return nil
		}})
	})
	return out, err
}

func sortSuggestions(items []struct{ Name, Email string }) {
	sort.SliceStable(items, func(i, j int) bool { return strings.ToLower(items[i].Email) < strings.ToLower(items[j].Email) })
}
func queryInt(v string) (int64, error) {
	if v == "" {
		return 0, nil
	}
	n, err := strconv.ParseInt(v, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("invalid numeric filter")
	}
	return n, nil
}
