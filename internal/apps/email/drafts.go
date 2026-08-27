package email

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func messageDomain(email string) string {
	if i := strings.LastIndex(email, "@"); i >= 0 {
		return email[i+1:]
	}
	return "dar.local"
}
func draftMessageID(acct Account) string {
	return fmt.Sprintf("%d.%s@%s", time.Now().UnixNano(), newID(), messageDomain(acct.Email))
}

func (a *App) ensureSpecialFolder(ctx context.Context, uid, accountID, role, name string) (Folder, error) {
	id, err := a.folderByRole(ctx, uid, accountID, role)
	if err == nil {
		return a.folderRecord(ctx, uid, id)
	}
	return a.createFolder(ctx, uid, accountID, name, role)
}

func (a *App) saveDraft(ctx context.Context, uid string, id int64, in DraftInput, extra *outgoingAttachment, removePart string) (Message, error) {
	acct, err := a.account(ctx, uid, in.AccountID)
	if err != nil {
		return Message{}, err
	}
	folder, err := a.ensureSpecialFolder(ctx, uid, acct.ID, "drafts", "Drafts")
	if err != nil {
		return Message{}, err
	}
	lock := a.lockFor(acct.ID)
	lock.Lock()
	defer lock.Unlock()
	msgID := draftMessageID(acct)
	var oldPath string
	var attachments []outgoingAttachment
	if id > 0 {
		oldPath, _, err = a.messagePath(ctx, uid, id)
		if err != nil {
			return Message{}, err
		}
		parsed, er := parseMailFile(oldPath)
		if er == nil {
			if parsed.HeaderMessageID != "" {
				msgID = strings.Trim(parsed.HeaderMessageID, "<>")
			}
			if in.To == "" {
				in.To = parsed.To
			}
			if in.Cc == "" {
				in.Cc = parsed.Cc
			}
			if in.Bcc == "" {
				in.Bcc = parsed.Bcc
			}
			if in.InReplyTo == "" {
				in.InReplyTo = parsed.InReplyTo
			}
			if in.References == "" {
				in.References = parsed.References
			}
			for _, x := range parsed.Attachments {
				if x.Part != removePart {
					attachments = append(attachments, outgoingAttachment{Filename: x.Filename, MIMEType: x.MIMEType, Data: x.Data, ContentID: x.ContentID, Inline: x.Inline})
				}
			}
		}
	}
	if extra != nil {
		attachments = append(attachments, *extra)
	}
	raw, _, err := buildMessage(outgoingMessage{FromName: acct.Name, FromEmail: acct.Email, To: in.To, Cc: in.Cc, Bcc: in.Bcc, Subject: in.Subject, Body: in.Body, MessageID: msgID, InReplyTo: in.InReplyTo, References: in.References, Attachments: attachments, IncludeBcc: true, Draft: true})
	if err != nil {
		return Message{}, err
	}
	dir := filepath.Join(folderDiskPath(a.mailRoot(uid, acct.ID), folder.Name), "cur")
	_ = os.MkdirAll(dir, 0700)
	filename := fmt.Sprintf("%d.%s:2,DS", time.Now().UnixNano(), newID())
	tmp := filepath.Join(dir, "."+filename+".tmp")
	dest := filepath.Join(dir, filename)
	if err := os.WriteFile(tmp, raw, 0600); err != nil {
		return Message{}, err
	}
	if err := os.Rename(tmp, dest); err != nil {
		return Message{}, err
	}
	if oldPath != "" {
		_ = os.Remove(oldPath)
	}
	if _, err := a.reindexAccount(ctx, uid, acct.ID); err != nil {
		return Message{}, err
	}
	var savedID int64
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT m.id FROM email_messages m JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND m.account_id=? AND m.header_message_id=? ORDER BY m.id DESC LIMIT 1`, &sqlitex.ExecOptions{Args: []any{uid, acct.ID, formatMessageID(msgID)}, ResultFunc: func(s *sqlite.Stmt) error {
			savedID = s.ColumnInt64(0)
			return nil
		}})
	})
	if err != nil {
		return Message{}, err
	}
	if savedID == 0 {
		return Message{}, errNotFound
	}
	return a.withMessage(ctx, uid, savedID)
}

func (a *App) deleteDraft(ctx context.Context, uid string, id int64) error {
	path, acct, err := a.messagePath(ctx, uid, id)
	if err != nil {
		return err
	}
	messages, err := a.withMessage(ctx, uid, id)
	if err != nil || !messages.Draft {
		return errors.New("message is not a draft")
	}
	lock := a.lockFor(acct.ID)
	lock.Lock()
	defer lock.Unlock()
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return err
	}
	_, err = a.reindexAccount(ctx, uid, acct.ID)
	return err
}

func (a *App) withMessage(ctx context.Context, uid string, id int64) (Message, error) {
	accountID, err := a.messageOwner(ctx, uid, id)
	if err != nil {
		return Message{}, err
	}
	var key string
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT thread_key FROM email_messages WHERE id=?`, &sqlitex.ExecOptions{Args: []any{id}, ResultFunc: func(s *sqlite.Stmt) error { key = s.ColumnText(0); return nil }})
	})
	if err != nil {
		return Message{}, err
	}
	list, err := a.threadMessages(ctx, uid, threadID(accountID, key))
	if err != nil {
		return Message{}, err
	}
	for _, m := range list {
		if m.ID == id {
			return m, nil
		}
	}
	return Message{}, errNotFound
}

func (a *App) sendDraft(ctx context.Context, uid string, id int64) error {
	path, acct, err := a.messagePath(ctx, uid, id)
	if err != nil {
		return err
	}
	lock := a.lockFor(acct.ID)
	lock.Lock()
	defer lock.Unlock()
	msg, err := a.withMessage(ctx, uid, id)
	if err != nil || !msg.Draft {
		return errors.New("message is not a draft")
	}
	parsed, err := parseMailFile(path)
	if err != nil {
		return err
	}
	var attachments []outgoingAttachment
	for _, x := range parsed.Attachments {
		attachments = append(attachments, outgoingAttachment{Filename: x.Filename, MIMEType: x.MIMEType, Data: x.Data, ContentID: x.ContentID, Inline: x.Inline})
	}
	raw, recipients, err := buildMessage(outgoingMessage{FromName: acct.Name, FromEmail: acct.Email, To: parsed.To, Cc: parsed.Cc, Bcc: parsed.Bcc, Subject: parsed.Subject, Body: parsed.Text, MessageID: strings.Trim(parsed.HeaderMessageID, "<>"), InReplyTo: parsed.InReplyTo, References: parsed.References, Attachments: attachments})
	if err != nil {
		return err
	}
	password, err := a.password(ctx, uid, acct)
	if err != nil {
		return err
	}
	if err := a.deliver(ctx, uid, acct, password, raw, recipients); err != nil {
		return err
	}
	sent, err := a.ensureSpecialFolder(ctx, uid, acct.ID, "sent", "Sent")
	if err != nil {
		return err
	}
	dir := filepath.Join(folderDiskPath(a.mailRoot(uid, acct.ID), sent.Name), "cur")
	_ = os.MkdirAll(dir, 0700)
	dest := filepath.Join(dir, fmt.Sprintf("%d.%s:2,S", time.Now().UnixNano(), newID()))
	if err := os.WriteFile(dest, raw, 0600); err != nil {
		return fmt.Errorf("message sent but Sent copy failed: %w", err)
	}
	_ = os.Remove(path)
	_, err = a.reindexAccount(ctx, uid, acct.ID)
	if err == nil {
		a.queueSync(uid, acct.ID, "operation")
	}
	return err
}
