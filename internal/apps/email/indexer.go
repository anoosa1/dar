package email

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func (a *App) accountRoot(uid, accountID string) string {
	return filepath.Join(a.dataRoot, uid, "email", "accounts", accountID)
}
func (a *App) mailRoot(uid, accountID string) string {
	return filepath.Join(a.accountRoot(uid, accountID), "mail")
}

func (a *App) managedMailBase(uid string) string {
	return filepath.Join(a.dataRoot, uid, "home", ".local", "var", "mail")
}

func (a *App) managedMailRoot(uid, email string) string {
	return filepath.Join(a.managedMailBase(uid), email)
}

// ensureManagedMailRoot atomically relocates the legacy account Maildir into
// the user's hidden Files tree and leaves a server-private relative symlink so
// existing email operations retain a stable account-ID based path.
func (a *App) ensureManagedMailRoot(uid string, acct Account) (string, error) {
	if acct.mailRootOverride != "" {
		if err := os.MkdirAll(acct.mailRootOverride, 0700); err != nil {
			return "", err
		}
		return acct.mailRootOverride, nil
	}
	base := a.managedMailBase(uid)
	target := a.managedMailRoot(uid, acct.Email)
	link := a.mailRoot(uid, acct.ID)
	if err := os.MkdirAll(base, 0700); err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Dir(link), 0700); err != nil {
		return "", err
	}

	info, err := os.Lstat(link)
	switch {
	case err == nil && info.Mode()&os.ModeSymlink != 0:
		destination, readErr := os.Readlink(link)
		if readErr != nil {
			return "", readErr
		}
		if !filepath.IsAbs(destination) {
			destination = filepath.Join(filepath.Dir(link), destination)
		}
		destination = filepath.Clean(destination)
		within, relErr := filepath.Rel(base, destination)
		if relErr != nil || within == ".." || strings.HasPrefix(within, ".."+string(filepath.Separator)) {
			return "", errors.New("Maildir link escapes managed mail root")
		}
		if destination == target {
			return target, nil
		}
		if _, statErr := os.Lstat(target); !os.IsNotExist(statErr) {
			return "", errors.New("target Maildir already exists")
		}
		if err := os.Rename(destination, target); err != nil {
			return "", err
		}
		if err := os.Remove(link); err != nil {
			_ = os.Rename(target, destination)
			return "", err
		}
	case err == nil && info.IsDir():
		if _, statErr := os.Lstat(target); !os.IsNotExist(statErr) {
			return "", errors.New("target Maildir already exists")
		}
		if err := os.Rename(link, target); err != nil {
			return "", err
		}
	case err == nil:
		return "", errors.New("legacy Maildir path is not a directory")
	case !os.IsNotExist(err):
		return "", err
	default:
		if _, statErr := os.Lstat(target); os.IsNotExist(statErr) {
			if err := os.Mkdir(target, 0700); err != nil {
				return "", err
			}
		} else if statErr != nil {
			return "", statErr
		}
	}

	relative, err := filepath.Rel(filepath.Dir(link), target)
	if err != nil {
		return "", err
	}
	if err := os.Symlink(relative, link); err != nil {
		return "", err
	}
	return target, nil
}

func (a *App) removeManagedMailRoot(uid string, acct Account) error {
	target := a.managedMailRoot(uid, acct.Email)
	if err := os.RemoveAll(target); err != nil {
		return err
	}
	return os.RemoveAll(a.accountRoot(uid, acct.ID))
}
func folderRole(name string) string {
	n := strings.ToLower(strings.Trim(name, "./"))
	switch {
	case n == "inbox":
		return "inbox"
	case strings.Contains(n, "sent"):
		return "sent"
	case strings.Contains(n, "draft"):
		return "drafts"
	case strings.Contains(n, "trash") || strings.Contains(n, "deleted"):
		return "trash"
	case strings.Contains(n, "archive") || strings.Contains(n, "all mail"):
		return "archive"
	case strings.Contains(n, "junk") || strings.Contains(n, "spam"):
		return "junk"
	}
	return ""
}
func displayFolder(root, path string) string {
	rel, _ := filepath.Rel(root, path)
	if rel == "INBOX" || rel == "." {
		return "INBOX"
	}
	rel = strings.TrimPrefix(rel, ".")
	return strings.ReplaceAll(rel, string(filepath.Separator), "/")
}

type newIndexedMessage struct {
	ContentHash string
	ThreadKey   string
	From        string
	Subject     string
	Folder      string
}

type indexStats struct {
	Scanned, Updated, Removed int
	NewMessages               []newIndexedMessage
}

func notificationFolderEligible(name string) bool {
	switch folderRole(name) {
	case "sent", "drafts", "trash", "junk":
		return false
	default:
		return true
	}
}

func (a *App) reindexAccount(ctx context.Context, uid, accountID string) (indexStats, error) {
	defer a.markSearchDirty()
	acct, err := a.account(ctx, uid, accountID)
	if err != nil {
		return indexStats{}, err
	}
	root, err := a.ensureManagedMailRoot(uid, acct)
	if err != nil {
		return indexStats{}, err
	}
	if err := os.MkdirAll(filepath.Join(root, "INBOX", "cur"), 0700); err != nil {
		return indexStats{}, err
	}
	_ = os.MkdirAll(filepath.Join(root, "INBOX", "new"), 0700)
	_ = os.MkdirAll(filepath.Join(root, "INBOX", "tmp"), 0700)
	type fileItem struct {
		folder, path, rel string
		info              fs.FileInfo
	}
	var files []fileItem
	folderPaths := map[string]string{}
	_ = filepath.Walk(root, func(path string, info fs.FileInfo, err error) error {
		if err != nil || info == nil {
			return nil
		}
		if info.IsDir() && (info.Name() == "cur" || info.Name() == "new") {
			parent := filepath.Dir(path)
			name := displayFolder(root, parent)
			folderPaths[name] = parent
			entries, _ := os.ReadDir(path)
			for _, e := range entries {
				if e.IsDir() {
					continue
				}
				fi, er := e.Info()
				if er != nil {
					continue
				}
				p := filepath.Join(path, e.Name())
				rel, _ := filepath.Rel(root, p)
				files = append(files, fileItem{name, p, rel, fi})
			}
			return filepath.SkipDir
		}
		return nil
	})
	sort.Slice(files, func(i, j int) bool { return files[i].rel < files[j].rel })
	stats := indexStats{Scanned: len(files)}
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return func() (txErr error) {
			end, txErr := sqlitex.ImmediateTransaction(c)
			if txErr != nil {
				return txErr
			}
			defer end(&txErr)
			folderIDs := map[string]int64{}
			for name := range folderPaths {
				if err := sqlitex.Execute(c, `INSERT INTO email_folders(account_id,name,role) VALUES(?,?,?) ON CONFLICT(account_id,name) DO UPDATE SET role=excluded.role`, &sqlitex.ExecOptions{Args: []any{accountID, name, folderRole(name)}}); err != nil {
					return err
				}
				if err := sqlitex.Execute(c, `SELECT id FROM email_folders WHERE account_id=? AND name=?`, &sqlitex.ExecOptions{Args: []any{accountID, name}, ResultFunc: func(s *sqlite.Stmt) error { folderIDs[name] = s.ColumnInt64(0); return nil }}); err != nil {
					return err
				}
			}
			seen := map[string]bool{}
			newMessages := map[int64]newIndexedMessage{}
			var newMessageOrder []int64
			for _, f := range files {
				fid := folderIDs[f.folder]
				seen[fmt.Sprintf("%d/%s", fid, f.rel)] = true
				unchanged := false
				if err := sqlitex.Execute(c, `SELECT file_size,file_mtime FROM email_message_locations WHERE folder_id=? AND maildir_path=?`, &sqlitex.ExecOptions{Args: []any{fid, f.rel}, ResultFunc: func(s *sqlite.Stmt) error {
					unchanged = s.ColumnInt64(0) == f.info.Size() && s.ColumnInt64(1) == f.info.ModTime().UnixNano()
					return nil
				}}); err != nil {
					return err
				}
				if unchanged {
					continue
				}
				parsed, err := parseMailFile(f.path)
				if err != nil {
					return err
				}
				var mid int64
				existed := false
				if err := sqlitex.Execute(c, `SELECT id FROM email_messages WHERE account_id=? AND content_hash=?`, &sqlitex.ExecOptions{Args: []any{accountID, parsed.Hash}, ResultFunc: func(s *sqlite.Stmt) error {
					mid = s.ColumnInt64(0)
					existed = true
					return nil
				}}); err != nil {
					return err
				}
				if err := sqlitex.Execute(c, `INSERT INTO email_messages(account_id,content_hash,header_message_id,thread_key,sender,recipients,cc,bcc,subject,sent_at,snippet,text_body,html_body,size,has_attachments,parse_error) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,content_hash) DO UPDATE SET header_message_id=excluded.header_message_id,thread_key=excluded.thread_key,sender=excluded.sender,recipients=excluded.recipients,cc=excluded.cc,bcc=excluded.bcc,subject=excluded.subject,sent_at=excluded.sent_at,snippet=excluded.snippet,text_body=excluded.text_body,html_body=excluded.html_body,size=excluded.size,has_attachments=excluded.has_attachments,parse_error=excluded.parse_error`, &sqlitex.ExecOptions{Args: []any{accountID, parsed.Hash, parsed.HeaderMessageID, parsed.ThreadKey, parsed.From, parsed.To, parsed.Cc, parsed.Bcc, parsed.Subject, parsed.Date, parsed.Snippet, parsed.Text, parsed.HTML, parsed.Size, boolInt(len(parsed.Attachments) > 0), parsed.ParseError}}); err != nil {
					return err
				}
				if err := sqlitex.Execute(c, `SELECT id FROM email_messages WHERE account_id=? AND content_hash=?`, &sqlitex.ExecOptions{Args: []any{accountID, parsed.Hash}, ResultFunc: func(s *sqlite.Stmt) error { mid = s.ColumnInt64(0); return nil }}); err != nil {
					return err
				}
				read, flagged, answered, draft, deleted := maildirFlags(filepath.Base(f.path))
				if err := sqlitex.Execute(c, `INSERT INTO email_message_locations(message_id,folder_id,maildir_path,file_size,file_mtime,is_read,is_flagged,is_answered,is_draft,is_deleted) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(folder_id,maildir_path) DO UPDATE SET message_id=excluded.message_id,file_size=excluded.file_size,file_mtime=excluded.file_mtime,is_read=excluded.is_read,is_flagged=excluded.is_flagged,is_answered=excluded.is_answered,is_draft=excluded.is_draft,is_deleted=excluded.is_deleted`, &sqlitex.ExecOptions{Args: []any{mid, fid, f.rel, f.info.Size(), f.info.ModTime().UnixNano(), boolInt(read), boolInt(flagged), boolInt(answered), boolInt(draft), boolInt(deleted)}}); err != nil {
					return err
				}
				if !existed {
					if _, tracked := newMessages[mid]; !tracked {
						newMessages[mid] = newIndexedMessage{
							ContentHash: parsed.Hash,
							ThreadKey:   parsed.ThreadKey,
							From:        parsed.From,
							Subject:     parsed.Subject,
						}
						newMessageOrder = append(newMessageOrder, mid)
					}
				}
				if candidate, tracked := newMessages[mid]; tracked && candidate.Folder == "" && notificationFolderEligible(f.folder) {
					candidate.Folder = f.folder
					newMessages[mid] = candidate
				}
				_ = sqlitex.Execute(c, `DELETE FROM email_recipients WHERE message_id=?`, &sqlitex.ExecOptions{Args: []any{mid}})
				for _, r := range parsed.Recipients {
					if err := sqlitex.Execute(c, `INSERT INTO email_recipients(message_id,kind,ordinal,name,email) VALUES(?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{mid, r.Kind, r.Ordinal, r.Name, r.Email}}); err != nil {
						return err
					}
				}
				_ = sqlitex.Execute(c, `DELETE FROM email_attachments WHERE message_id=?`, &sqlitex.ExecOptions{Args: []any{mid}})
				var filenames []string
				for _, att := range parsed.Attachments {
					filenames = append(filenames, att.Filename)
					if err := sqlitex.Execute(c, `INSERT INTO email_attachments(message_id,part,filename,mime_type,size,content_id,is_inline) VALUES(?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{mid, att.Part, att.Filename, att.MIMEType, att.Size, att.ContentID, boolInt(att.Inline)}}); err != nil {
						return err
					}
				}
				_ = sqlitex.Execute(c, `DELETE FROM email_fts WHERE message_id=?`, &sqlitex.ExecOptions{Args: []any{mid}})
				if err := sqlitex.Execute(c, `INSERT INTO email_fts(message_id,user_id,subject,participants,body,filenames) VALUES(?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{mid, uid, parsed.Subject, parsed.From + " " + parsed.To + " " + parsed.Cc + " " + parsed.Bcc, parsed.Text, strings.Join(filenames, " ")}}); err != nil {
					return err
				}
				stats.Updated++
			}
			var stale []struct{ id, mid int64 }
			if err := sqlitex.Execute(c, `SELECT l.id,l.message_id,l.folder_id,l.maildir_path FROM email_message_locations l JOIN email_folders f ON f.id=l.folder_id WHERE f.account_id=?`, &sqlitex.ExecOptions{Args: []any{accountID}, ResultFunc: func(s *sqlite.Stmt) error {
				key := fmt.Sprintf("%d/%s", s.ColumnInt64(2), s.ColumnText(3))
				if !seen[key] {
					stale = append(stale, struct{ id, mid int64 }{s.ColumnInt64(0), s.ColumnInt64(1)})
				}
				return nil
			}}); err != nil {
				return err
			}
			for _, x := range stale {
				_ = sqlitex.Execute(c, `DELETE FROM email_message_locations WHERE id=?`, &sqlitex.ExecOptions{Args: []any{x.id}})
				stats.Removed++
			}
			var orphanIDs []int64
			_ = sqlitex.Execute(c, `SELECT m.id FROM email_messages m WHERE m.account_id=? AND NOT EXISTS(SELECT 1 FROM email_message_locations l WHERE l.message_id=m.id)`, &sqlitex.ExecOptions{Args: []any{accountID}, ResultFunc: func(s *sqlite.Stmt) error { orphanIDs = append(orphanIDs, s.ColumnInt64(0)); return nil }})
			for _, id := range orphanIDs {
				_ = sqlitex.Execute(c, `DELETE FROM email_fts WHERE message_id=?`, &sqlitex.ExecOptions{Args: []any{id}})
				_ = sqlitex.Execute(c, `DELETE FROM email_messages WHERE id=?`, &sqlitex.ExecOptions{Args: []any{id}})
			}
			var staleFolderIDs []int64
			_ = sqlitex.Execute(c, `SELECT id,name FROM email_folders WHERE account_id=?`, &sqlitex.ExecOptions{Args: []any{accountID}, ResultFunc: func(s *sqlite.Stmt) error {
				if _, exists := folderPaths[s.ColumnText(1)]; !exists {
					staleFolderIDs = append(staleFolderIDs, s.ColumnInt64(0))
				}
				return nil
			}})
			for _, id := range staleFolderIDs {
				_ = sqlitex.Execute(c, `DELETE FROM email_folders WHERE id=?`, &sqlitex.ExecOptions{Args: []any{id}})
			}
			_ = sqlitex.Execute(c, `UPDATE email_folders SET total_count=(SELECT count(*) FROM email_message_locations l WHERE l.folder_id=email_folders.id),unread_count=(SELECT count(*) FROM email_message_locations l WHERE l.folder_id=email_folders.id AND l.is_read=0) WHERE account_id=?`, &sqlitex.ExecOptions{Args: []any{accountID}})
			for _, id := range newMessageOrder {
				if candidate := newMessages[id]; candidate.Folder != "" {
					stats.NewMessages = append(stats.NewMessages, candidate)
				}
			}
			return nil
		}()
	})
	return stats, err
}

func maildirFlags(name string) (read, flagged, answered, draft, deleted bool) {
	i := strings.LastIndex(name, ":2,")
	if i < 0 {
		return
	}
	flags := name[i+3:]
	return strings.Contains(flags, "S"), strings.Contains(flags, "F"), strings.Contains(flags, "R"), strings.Contains(flags, "D"), strings.Contains(flags, "T")
}
