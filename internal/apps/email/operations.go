package email

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type operationPayload struct {
	MessageIDs        []int64 `json:"message_ids,omitempty"`
	SourceFolder      int64   `json:"source_folder,omitempty"`
	DestinationFolder int64   `json:"destination_folder,omitempty"`
	SourceName        string  `json:"source_name,omitempty"`
	DestinationName   string  `json:"destination_name,omitempty"`
}

func (a *App) journalOperation(ctx context.Context, accountID, kind string, p operationPayload) error {
	raw, _ := json.Marshal(p)
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_operations(account_id,kind,payload,phase,state,created_at,updated_at) VALUES(?,?,?,'local-applied','pending',?,?)`, &sqlitex.ExecOptions{Args: []any{accountID, kind, string(raw), nowUnix(), nowUnix()}})
	})
}

type locationFile struct {
	MessageID, LocationID, FolderID int64
	AccountID, Rel, Role            string
}

func (a *App) locationFiles(ctx context.Context, uid string, ids []int64) ([]locationFile, error) {
	if len(ids) == 0 {
		return nil, errors.New("message_ids is required")
	}
	args := []any{uid}
	for _, id := range ids {
		args = append(args, id)
	}
	var out []locationFile
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT m.id,l.id,l.folder_id,m.account_id,l.maildir_path,f.role FROM email_messages m JOIN email_accounts a ON a.id=m.account_id JOIN email_message_locations l ON l.message_id=m.id JOIN email_folders f ON f.id=l.folder_id WHERE a.user_id=? AND m.id IN (`+sqlPlaceholders(len(ids))+`)`, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, locationFile{s.ColumnInt64(0), s.ColumnInt64(1), s.ColumnInt64(2), s.ColumnText(3), s.ColumnText(4), s.ColumnText(5)})
			return nil
		}})
	})
	if err == nil && len(out) == 0 {
		err = errNotFound
	}
	return out, err
}

func updateMaildirFlags(path string, set, clear byte) (string, error) {
	base := filepath.Base(path)
	dir := filepath.Dir(path)
	i := strings.LastIndex(base, ":2,")
	stem, flags := base, ""
	if i >= 0 {
		stem = base[:i]
		flags = base[i+3:]
	}
	m := map[byte]bool{}
	for i := 0; i < len(flags); i++ {
		m[flags[i]] = true
	}
	if set != 0 {
		m[set] = true
	}
	if clear != 0 {
		delete(m, clear)
	}
	var keys []int
	for k := range m {
		keys = append(keys, int(k))
	}
	sort.Ints(keys)
	var b strings.Builder
	for _, k := range keys {
		b.WriteByte(byte(k))
	}
	destDir := dir
	if set == 'S' && filepath.Base(dir) == "new" {
		destDir = filepath.Join(filepath.Dir(dir), "cur")
		_ = os.MkdirAll(destDir, 0700)
	}
	newPath := filepath.Join(destDir, stem+":2,"+b.String())
	if path == newPath {
		return path, nil
	}
	if err := os.Rename(path, newPath); err != nil {
		return "", err
	}
	return newPath, nil
}

func (a *App) applyMessageAction(ctx context.Context, uid, action string, ids []int64, destination int64) error {
	defer a.markSearchDirty()
	files, err := a.locationFiles(ctx, uid, ids)
	if err != nil {
		return err
	}
	accountID := files[0].AccountID
	for _, f := range files {
		if f.AccountID != accountID {
			return errors.New("cross-account actions are not supported")
		}
	}
	lock := a.lockFor(accountID)
	lock.Lock()
	defer lock.Unlock()
	switch action {
	case "read", "unread", "flag", "unflag":
		set, clear := byte(0), byte(0)
		if action == "read" {
			set = 'S'
		} else if action == "unread" {
			clear = 'S'
		} else if action == "flag" {
			set = 'F'
		} else {
			clear = 'F'
		}
		for _, f := range files {
			path := filepath.Join(a.mailRoot(uid, f.AccountID), f.Rel)
			if _, err := updateMaildirFlags(path, set, clear); err != nil && !os.IsNotExist(err) {
				return err
			}
		}
		_ = a.journalOperation(ctx, accountID, action, operationPayload{MessageIDs: ids})
	case "archive", "trash", "move":
		if action == "archive" {
			destination, err = a.folderByRole(ctx, uid, accountID, "archive")
		} else if action == "trash" {
			destination, err = a.folderByRole(ctx, uid, accountID, "trash")
		}
		if err != nil {
			return err
		}
		if destination == 0 {
			return errors.New("destination folder is required")
		}
		if err := a.moveFiles(ctx, uid, files, destination); err != nil {
			return err
		}
		_ = a.journalOperation(ctx, accountID, action, operationPayload{MessageIDs: ids, DestinationFolder: destination})
	case "delete":
		for _, f := range files {
			if f.Role != "trash" {
				return errors.New("permanent deletion is only allowed in Trash")
			}
			if err := os.Remove(filepath.Join(a.mailRoot(uid, f.AccountID), f.Rel)); err != nil && !os.IsNotExist(err) {
				return err
			}
		}
		_ = a.journalOperation(ctx, accountID, action, operationPayload{MessageIDs: ids})
	default:
		return errors.New("unknown message action")
	}
	if _, err := a.reindexAccount(ctx, uid, accountID); err != nil {
		return err
	}
	a.queueSync(uid, accountID, "operation")
	return nil
}

func (a *App) folderByRole(ctx context.Context, uid, accountID, role string) (int64, error) {
	var id int64
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT f.id FROM email_folders f JOIN email_accounts a ON a.id=f.account_id WHERE a.user_id=? AND f.account_id=? AND f.role=? LIMIT 1`, &sqlitex.ExecOptions{Args: []any{uid, accountID, role}, ResultFunc: func(s *sqlite.Stmt) error { id = s.ColumnInt64(0); return nil }})
	})
	if err == nil && id == 0 {
		err = fmt.Errorf("%s folder is not mapped", role)
	}
	return id, err
}
func (a *App) folderRecord(ctx context.Context, uid string, id int64) (Folder, error) {
	var f Folder
	found := false
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT f.id,f.account_id,f.name,f.role,f.total_count,f.unread_count FROM email_folders f JOIN email_accounts a ON a.id=f.account_id WHERE a.user_id=? AND f.id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error {
			f = Folder{ID: s.ColumnInt64(0), AccountID: s.ColumnText(1), Name: s.ColumnText(2), Role: s.ColumnText(3), Total: int(s.ColumnInt64(4)), Unread: int(s.ColumnInt64(5))}
			found = true
			return nil
		}})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return f, err
}
func folderDiskPath(root, name string) string {
	if strings.EqualFold(name, "INBOX") {
		return filepath.Join(root, "INBOX")
	}
	return filepath.Join(root, "."+strings.ReplaceAll(strings.Trim(name, "./"), "/", string(filepath.Separator)))
}
func (a *App) moveFiles(ctx context.Context, uid string, files []locationFile, destination int64) error {
	dest, err := a.folderRecord(ctx, uid, destination)
	if err != nil {
		return err
	}
	for _, f := range files {
		if f.AccountID != dest.AccountID {
			return errors.New("cross-account moves are not supported")
		}
		src := filepath.Join(a.mailRoot(uid, f.AccountID), f.Rel)
		dstDir := filepath.Join(folderDiskPath(a.mailRoot(uid, f.AccountID), dest.Name), "cur")
		_ = os.MkdirAll(dstDir, 0700)
		dst := filepath.Join(dstDir, filepath.Base(src))
		if err := os.Rename(src, dst); err != nil && !os.IsNotExist(err) {
			return err
		}
	}
	return nil
}

func (a *App) createFolder(ctx context.Context, uid, accountID, name, role string) (Folder, error) {
	if _, err := a.account(ctx, uid, accountID); err != nil {
		return Folder{}, err
	}
	name = strings.Trim(strings.TrimSpace(name), "./")
	if name == "" || strings.Contains(name, "..") || strings.ContainsAny(name, "\x00\\") {
		return Folder{}, errors.New("invalid folder name")
	}
	if role != "" && folderRole(role) == "" && role != "inbox" {
		return Folder{}, errors.New("invalid folder role")
	}
	dir := folderDiskPath(a.mailRoot(uid, accountID), name)
	for _, sub := range []string{"cur", "new", "tmp"} {
		if err := os.MkdirAll(filepath.Join(dir, sub), 0700); err != nil {
			return Folder{}, err
		}
	}
	if err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_folders(account_id,name,role) VALUES(?,?,?)`, &sqlitex.ExecOptions{Args: []any{accountID, name, role}})
	}); err != nil {
		return Folder{}, err
	}
	_ = a.journalOperation(ctx, accountID, "folder-create", operationPayload{DestinationName: name})
	a.queueSync(uid, accountID, "operation")
	folders, _ := a.folders(ctx, uid, accountID)
	for _, f := range folders {
		if f.Name == name {
			return f, nil
		}
	}
	return Folder{}, errNotFound
}

func (a *App) renameFolder(ctx context.Context, uid string, id int64, name, role string) (Folder, error) {
	f, err := a.folderRecord(ctx, uid, id)
	if err != nil {
		return Folder{}, err
	}
	if f.Role == "inbox" {
		return Folder{}, errors.New("Inbox cannot be renamed")
	}
	lock := a.lockFor(f.AccountID)
	lock.Lock()
	defer lock.Unlock()
	name = strings.Trim(strings.TrimSpace(name), "./")
	if name == "" || strings.Contains(name, "..") {
		return Folder{}, errors.New("invalid folder name")
	}
	root := a.mailRoot(uid, f.AccountID)
	src, dst := folderDiskPath(root, f.Name), folderDiskPath(root, name)
	for _, sub := range []string{"cur", "new", "tmp"} {
		_ = os.MkdirAll(filepath.Join(dst, sub), 0700)
	}
	for _, sub := range []string{"cur", "new"} {
		entries, _ := os.ReadDir(filepath.Join(src, sub))
		for _, e := range entries {
			if !e.IsDir() {
				_ = os.Rename(filepath.Join(src, sub, e.Name()), filepath.Join(dst, sub, e.Name()))
			}
		}
	}
	if err := os.RemoveAll(src); err != nil {
		return Folder{}, err
	}
	err = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE email_folders SET name=?,role=? WHERE id=?`, &sqlitex.ExecOptions{Args: []any{name, role, id}})
	})
	if err != nil {
		return Folder{}, err
	}
	_ = a.journalOperation(ctx, f.AccountID, "folder-rename", operationPayload{SourceFolder: id, SourceName: f.Name, DestinationName: name})
	if _, err := a.reindexAccount(ctx, uid, f.AccountID); err != nil {
		return Folder{}, err
	}
	a.queueSync(uid, f.AccountID, "operation")
	return a.folderRecord(ctx, uid, id)
}

func (a *App) deleteFolder(ctx context.Context, uid string, id int64, deleteMessages bool) error {
	f, err := a.folderRecord(ctx, uid, id)
	if err != nil {
		return err
	}
	if f.Role == "inbox" {
		return errors.New("Inbox cannot be deleted")
	}
	lock := a.lockFor(f.AccountID)
	lock.Lock()
	defer lock.Unlock()
	if f.Total > 0 && !deleteMessages {
		return errors.New("folder is not empty")
	}
	dir := folderDiskPath(a.mailRoot(uid, f.AccountID), f.Name)
	if deleteMessages {
		for _, sub := range []string{"cur", "new"} {
			entries, _ := os.ReadDir(filepath.Join(dir, sub))
			for _, e := range entries {
				if !e.IsDir() {
					_ = os.Remove(filepath.Join(dir, sub, e.Name()))
				}
			}
		}
	}
	_ = a.journalOperation(ctx, f.AccountID, "folder-delete", operationPayload{SourceFolder: id, SourceName: f.Name})
	_ = os.RemoveAll(dir)
	_ = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM email_folders WHERE id=?`, &sqlitex.ExecOptions{Args: []any{id}})
	})
	if _, err := a.reindexAccount(ctx, uid, f.AccountID); err != nil {
		return err
	}
	a.queueSync(uid, f.AccountID, "operation")
	return nil
}
