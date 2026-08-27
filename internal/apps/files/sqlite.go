package files

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS files_favorites (user_id TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(user_id,path));`,
	`CREATE TABLE IF NOT EXISTS files_tags (user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, color TEXT NOT NULL, PRIMARY KEY(user_id,id));`,
	`CREATE TABLE IF NOT EXISTS files_file_tags (user_id TEXT NOT NULL, path TEXT NOT NULL, tag_id TEXT NOT NULL, PRIMARY KEY(user_id,path,tag_id));`,
	`CREATE TABLE IF NOT EXISTS files_trash (user_id TEXT NOT NULL, id TEXT NOT NULL, original_path TEXT NOT NULL, trash_path TEXT NOT NULL, deleted_at INTEGER NOT NULL, PRIMARY KEY(user_id,id));`,
	`CREATE TABLE IF NOT EXISTS files_versions (user_id TEXT NOT NULL, path TEXT NOT NULL, id TEXT NOT NULL, blob_path TEXT NOT NULL, size INTEGER NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(user_id,path,id));`,
	`CREATE TABLE IF NOT EXISTS files_public_shares (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, path TEXT NOT NULL, password_salt BLOB NOT NULL DEFAULT X'', password_hash BLOB NOT NULL DEFAULT X'', expires INTEGER NOT NULL DEFAULT 0, drop_only INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);`,
	`CREATE INDEX IF NOT EXISTS idx_files_public_shares_user ON files_public_shares(user_id,created_at DESC);`,
	`CREATE TABLE IF NOT EXISTS files_user_shares (id TEXT PRIMARY KEY, from_user TEXT NOT NULL, to_user TEXT NOT NULL, path TEXT NOT NULL, permission TEXT NOT NULL, created_at INTEGER NOT NULL);`,
	`CREATE INDEX IF NOT EXISTS idx_files_user_shares_users ON files_user_shares(from_user,to_user);`,
	`CREATE TABLE IF NOT EXISTS files_comments (user_id TEXT NOT NULL, path TEXT NOT NULL, id TEXT NOT NULL, author TEXT NOT NULL, body TEXT NOT NULL, parent_id TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, PRIMARY KEY(user_id,path,id));`,
	`CREATE TABLE IF NOT EXISTS files_locks (user_id TEXT NOT NULL, path TEXT NOT NULL, owner TEXT NOT NULL, PRIMARY KEY(user_id,path));`,
	`CREATE TABLE IF NOT EXISTS files_activity (user_id TEXT NOT NULL, id TEXT NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, action TEXT NOT NULL, happened_at INTEGER NOT NULL, PRIMARY KEY(user_id,id));`,
	`CREATE INDEX IF NOT EXISTS idx_files_activity_recent ON files_activity(user_id,happened_at DESC);`,
	`CREATE TABLE IF NOT EXISTS files_external_storage (user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, type TEXT NOT NULL, config_json TEXT NOT NULL, mount_path TEXT NOT NULL, PRIMARY KEY(user_id,id));`,
	`CREATE TABLE IF NOT EXISTS files_operation_leases (user_id TEXT NOT NULL, path TEXT NOT NULL, token TEXT NOT NULL, expires_at INTEGER NOT NULL, PRIMARY KEY(user_id,path));`,
	`DROP TABLE IF EXISTS files_external_storage;`,
}

func Migrations() []string { return migrations }

var ErrLocked = errors.New("file path is locked")
var ErrManagedPathReadOnly = errors.New("managed Maildir is read-only in Files")
var ErrShareReadOnly = errors.New("shared path is read-only")
var ErrSharedRootMutation = errors.New("the root of a shared mount cannot be changed")

type resolvedUserPath struct {
	OwnerUID   string
	OwnerPath  string
	Virtual    string
	ShareID    string
	Permission string
}

func (app *App) resolveUserPath(ctx context.Context, uid, vpath string, write bool) (resolvedUserPath, error) {
	vpath = app.cleanVPath(vpath)
	resolved := resolvedUserPath{OwnerUID: uid, OwnerPath: vpath, Virtual: vpath, Permission: "write"}
	if vpath != "/Shared" && !strings.HasPrefix(vpath, "/Shared/") {
		return resolved, nil
	}
	parts := strings.Split(strings.TrimPrefix(vpath, "/Shared/"), "/")
	if vpath == "/Shared" || len(parts) == 0 || parts[0] == "" {
		return resolvedUserPath{}, os.ErrNotExist
	}
	var share UserShare
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,path,from_user,to_user,permission,created_at FROM files_user_shares WHERE id=? AND to_user=?`, &sqlitex.ExecOptions{
			Args: []any{parts[0], uid},
			ResultFunc: func(s *sqlite.Stmt) error {
				share = UserShare{ID: s.ColumnText(0), Path: s.ColumnText(1), FromUser: s.ColumnText(2), ToUser: s.ColumnText(3), Permission: s.ColumnText(4), Created: s.ColumnInt64(5)}
				return nil
			},
		})
	})
	if err != nil {
		return resolvedUserPath{}, err
	}
	if share.ID == "" {
		return resolvedUserPath{}, os.ErrNotExist
	}
	if write && share.Permission != "write" {
		return resolvedUserPath{}, ErrShareReadOnly
	}
	if write && len(parts) == 1 {
		return resolvedUserPath{}, ErrSharedRootMutation
	}
	ownerPath := app.cleanVPath(share.Path)
	if len(parts) > 1 {
		ownerPath = app.cleanVPath(path.Join(ownerPath, path.Join(parts[1:]...)))
	}
	return resolvedUserPath{OwnerUID: share.FromUser, OwnerPath: ownerPath, Virtual: vpath, ShareID: share.ID, Permission: share.Permission}, nil
}

func (app *App) managedPathReadOnly(vpath string) bool {
	vpath = app.cleanVPath(vpath)
	const mailRoot = "/.local/var/mail"
	return vpath == mailRoot || strings.HasPrefix(vpath, mailRoot+"/")
}

func (app *App) allowFilesMutation(paths ...string) error {
	for _, vpath := range paths {
		if app.managedPathReadOnly(vpath) {
			return ErrManagedPathReadOnly
		}
	}
	return nil
}

type Version struct {
	ID   string `json:"id"`
	Size int64  `json:"size"`
	Time int64  `json:"time"`
}

type trashRecord struct {
	ID, OriginalPath, TrashPath string
	DeletedAt                   int64
}

func (app *App) trashRecords(ctx context.Context, uid string) ([]trashRecord, error) {
	var out []trashRecord
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,original_path,trash_path,deleted_at FROM files_trash WHERE user_id=? ORDER BY deleted_at DESC`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, trashRecord{ID: s.ColumnText(0), OriginalPath: s.ColumnText(1), TrashPath: s.ColumnText(2), DeletedAt: s.ColumnInt64(3)})
			return nil
		}})
	})
	return out, err
}

func (app *App) trashRecord(ctx context.Context, uid, id string) (trashRecord, error) {
	var out trashRecord
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,original_path,trash_path,deleted_at FROM files_trash WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error {
			out = trashRecord{ID: s.ColumnText(0), OriginalPath: s.ColumnText(1), TrashPath: s.ColumnText(2), DeletedAt: s.ColumnInt64(3)}
			return nil
		}})
	})
	if err != nil {
		return out, err
	}
	if out.ID == "" {
		return out, os.ErrNotExist
	}
	return out, nil
}

func (app *App) putTrashRecord(ctx context.Context, uid string, r trashRecord) error {
	return app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO files_trash(user_id,id,original_path,trash_path,deleted_at) VALUES(?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, r.ID, r.OriginalPath, r.TrashPath, r.DeletedAt}})
	})
}
func (app *App) deleteTrashRecord(ctx context.Context, uid, id string) error {
	return app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM files_trash WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
	})
}

func (app *App) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if app.db == nil {
		return errors.New("files database is not configured")
	}
	c, release, err := app.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}

func overlaps(a, b string) bool {
	a, b = path.Clean("/"+strings.TrimPrefix(a, "/")), path.Clean("/"+strings.TrimPrefix(b, "/"))
	return a == b || a == "/" || b == "/" || strings.HasPrefix(a, b+"/") || strings.HasPrefix(b, a+"/")
}

func leaseToken() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

func (app *App) acquireLeases(ctx context.Context, uid string, paths []string, ttl time.Duration) (string, error) {
	cleaned := make([]string, 0, len(paths))
	seen := make(map[string]bool, len(paths))
	for _, p := range paths {
		p = app.cleanVPath(p)
		if p != "/" && !seen[p] {
			cleaned = append(cleaned, p)
			seen[p] = true
		}
	}
	sort.Strings(cleaned)
	token, now := leaseToken(), time.Now()
	err := app.withConn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM files_operation_leases WHERE expires_at<=?`, &sqlitex.ExecOptions{Args: []any{now.UnixMilli()}}); err != nil {
			return err
		}
		var held []string
		if err = sqlitex.Execute(c, `SELECT path FROM files_operation_leases WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { held = append(held, s.ColumnText(0)); return nil }}); err != nil {
			return err
		}
		for _, requested := range cleaned {
			for _, existing := range held {
				if overlaps(requested, existing) {
					return ErrLocked
				}
			}
			if err = sqlitex.Execute(c, `INSERT INTO files_operation_leases(user_id,path,token,expires_at) VALUES(?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, requested, token, now.Add(ttl).UnixMilli()}}); err != nil {
				return err
			}
			held = append(held, requested)
		}
		return nil
	})
	return token, err
}

func (app *App) releaseLeases(ctx context.Context, token string) {
	_ = app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM files_operation_leases WHERE token=?`, &sqlitex.ExecOptions{Args: []any{token}})
	})
}

// ReadFile is the error-returning file capability consumed by Notes.
func (app *App) ReadFile(ctx context.Context, uid, vpath string) ([]byte, error) {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, false)
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return nil, err
	}
	defer root.Close()
	return root.ReadFile(strings.TrimPrefix(resolved.OwnerPath, "/"))
}

func (app *App) Stat(ctx context.Context, uid, vpath string) (os.FileInfo, error) {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, false)
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return nil, err
	}
	defer root.Close()
	name := strings.TrimPrefix(resolved.OwnerPath, "/")
	if name == "" {
		name = "."
	}
	return root.Stat(name)
}

func (app *App) Walk(ctx context.Context, uid, root string, fn filepath.WalkFunc) error {
	resolved, err := app.resolveUserPath(ctx, uid, root, false)
	if err != nil {
		return err
	}
	userRoot, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return err
	}
	defer userRoot.Close()
	start := strings.TrimPrefix(resolved.OwnerPath, "/")
	if start == "" {
		start = "."
	}
	return fs.WalkDir(userRoot.FS(), start, func(name string, entry fs.DirEntry, walkErr error) error {
		virtual := app.cleanVPath(path.Join(resolved.Virtual, strings.TrimPrefix(filepath.ToSlash(name), strings.TrimPrefix(resolved.OwnerPath, "/"))))
		if walkErr != nil {
			return fn(virtual, nil, walkErr)
		}
		info, infoErr := entry.Info()
		return fn(virtual, info, infoErr)
	})
}

func (app *App) MkdirAll(ctx context.Context, uid, vpath string) error {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(resolved.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, resolved.OwnerUID, []string{resolved.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	defer app.markSearchDirty()
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return err
	}
	defer root.Close()
	return root.MkdirAll(strings.TrimPrefix(resolved.OwnerPath, "/"), 0700)
}

func (app *App) WriteFile(ctx context.Context, uid, vpath string, data []byte) error {
	return app.WriteStream(ctx, uid, vpath, bytes.NewReader(data))
}

func (app *App) CreateFile(ctx context.Context, uid, vpath string) error {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(resolved.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, resolved.OwnerUID, []string{resolved.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return err
	}
	defer root.Close()
	name := strings.TrimPrefix(resolved.OwnerPath, "/")
	if err := root.MkdirAll(filepath.Dir(name), 0700); err != nil {
		return err
	}
	file, err := root.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	if err := file.Close(); err != nil {
		return err
	}
	app.markSearchDirty()
	return nil
}

// CreateSymlink creates a user-visible link while guaranteeing that both the
// link and its eventual target remain inside the user's Files root.
func (app *App) CreateSymlink(ctx context.Context, uid, vpath, target string) error {
	vpath = app.cleanVPath(vpath)
	target = strings.TrimSpace(target)
	if vpath == "/" || target == "" || strings.ContainsRune(target, '\x00') {
		return fmt.Errorf("link path and target are required")
	}
	if err := app.allowFilesMutation(vpath); err != nil {
		return err
	}
	var resolved string
	if strings.HasPrefix(target, "/") {
		resolved = app.cleanVPath(target)
		if resolved != target {
			return fmt.Errorf("target must be a normalized user-rooted path")
		}
	} else {
		resolved = app.cleanVPath(path.Join(path.Dir(vpath), path.Clean(target)))
	}
	if resolved == "/Shared" || strings.HasPrefix(resolved, "/Shared/") || app.managedPathReadOnly(resolved) {
		return fmt.Errorf("links to virtual shares or managed Maildir paths are not allowed")
	}
	relative, err := filepath.Rel(filepath.FromSlash(path.Dir(vpath)), filepath.FromSlash(resolved))
	if err != nil {
		return err
	}
	storedTarget := filepath.ToSlash(relative)

	token, err := app.acquireLeases(ctx, uid, []string{vpath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	root, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		return err
	}
	defer root.Close()
	name := strings.TrimPrefix(vpath, "/")
	if err := root.MkdirAll(filepath.Dir(name), 0700); err != nil {
		return err
	}
	if err := root.Symlink(filepath.FromSlash(storedTarget), filepath.FromSlash(name)); err != nil {
		return err
	}
	app.markSearchDirty()
	return nil
}

// WriteStream writes user content through a same-directory temporary file,
// syncs it, and atomically replaces the destination without buffering it all.
func (app *App) WriteStream(ctx context.Context, uid, vpath string, source io.Reader) error {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(resolved.OwnerPath); err != nil {
		return err
	}
	if app.quotaBytes > 0 {
		app.quotaMu.Lock()
		defer app.quotaMu.Unlock()
	}
	token, err := app.acquireLeases(ctx, resolved.OwnerUID, []string{resolved.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	defer app.markSearchDirty()
	return app.writeStreamUnlocked(resolved.OwnerUID, resolved.OwnerPath, source)
}

// writeStreamUnlocked performs the atomic filesystem replacement after the
// caller has acquired the relevant operation lease and quota mutex.
func (app *App) writeStreamUnlocked(uid, vpath string, source io.Reader) error {
	root, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		return err
	}
	defer root.Close()
	dst := strings.TrimPrefix(app.cleanVPath(vpath), "/")
	parent := filepath.Dir(dst)
	if err := root.MkdirAll(parent, 0700); err != nil {
		return err
	}
	tmpName := filepath.Join(parent, ".dar-write-"+leaseToken())
	tmp, err := root.OpenFile(tmpName, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer root.Remove(tmpName)
	reader := source
	var allowed int64 = -1
	if app.quotaBytes > 0 {
		used := app.storageUsage(uid)
		if current, statErr := root.Stat(dst); statErr == nil {
			used -= current.Size()
		}
		allowed = app.quotaBytes - used
		if allowed < 0 {
			allowed = 0
		}
		reader = io.LimitReader(source, allowed+1)
	}
	buffer := make([]byte, 64<<10)
	written, copyErr := io.CopyBuffer(tmp, reader, buffer)
	if copyErr != nil {
		_ = tmp.Close()
		return copyErr
	}
	if allowed >= 0 && written > allowed {
		_ = tmp.Close()
		return fmt.Errorf("user storage quota exceeded")
	}
	if err = tmp.Sync(); err != nil {
		_ = tmp.Close()
		return err
	}
	if err = tmp.Close(); err != nil {
		return err
	}
	return root.Rename(tmpName, dst)
}

func (app *App) storageUsage(uid string) int64 {
	var total int64
	_ = filepath.Walk(filepath.Join(app.dataRoot, uid), func(_ string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() {
			total += info.Size()
		}
		return nil
	})
	return total
}

func (app *App) Rename(ctx context.Context, uid, from, to string) error {
	source, err := app.resolveUserPath(ctx, uid, from, true)
	if err != nil {
		return err
	}
	destination, err := app.resolveUserPath(ctx, uid, to, true)
	if err != nil {
		return err
	}
	if source.OwnerUID != destination.OwnerUID || source.ShareID != destination.ShareID {
		return fmt.Errorf("move across Files roots is not supported; use copy")
	}
	if err := app.allowFilesMutation(source.OwnerPath, destination.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, source.OwnerUID, []string{source.OwnerPath, destination.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	defer app.markSearchDirty()
	return app.renameUnlocked(ctx, source.OwnerUID, source.OwnerPath, destination.OwnerPath)
}

// renameUnlocked renames a path and its metadata while the caller owns leases.
func (app *App) renameUnlocked(ctx context.Context, uid, from, to string) error {
	root, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		return err
	}
	defer root.Close()
	fromName, toName := strings.TrimPrefix(app.cleanVPath(from), "/"), strings.TrimPrefix(app.cleanVPath(to), "/")
	if err := root.MkdirAll(filepath.Dir(toName), 0700); err != nil {
		return err
	}
	if err := root.Rename(fromName, toName); err != nil {
		return err
	}
	if err := app.movePathMetadata(ctx, uid, from, to); err != nil {
		_ = root.Rename(toName, fromName)
		return err
	}
	return nil
}

func (app *App) Remove(ctx context.Context, uid, vpath string) error {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(resolved.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, resolved.OwnerUID, []string{resolved.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	defer app.markSearchDirty()
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return err
	}
	defer root.Close()
	if err := root.RemoveAll(strings.TrimPrefix(resolved.OwnerPath, "/")); err != nil {
		return err
	}
	return app.purgePathMetadata(ctx, resolved.OwnerUID, resolved.OwnerPath)
}

func (app *App) SetModTime(ctx context.Context, uid, vpath string, modified time.Time) error {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(resolved.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, resolved.OwnerUID, []string{resolved.OwnerPath}, 20*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	defer app.markSearchDirty()
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return err
	}
	defer root.Close()
	return root.Chtimes(strings.TrimPrefix(resolved.OwnerPath, "/"), modified, modified)
}

func pathMatchSQL(column string) string {
	return `(` + column + `=? OR ` + column + ` LIKE ? ESCAPE '\')`
}
func pathArgs(vpath string) []any {
	pattern := strings.NewReplacer(`\`, `\\`, `%`, `\%`, `_`, `\_`).Replace(vpath)
	return []any{vpath, pattern + `/%`}
}
func (app *App) movePathMetadata(ctx context.Context, uid, from, to string) error {
	from, to = app.cleanVPath(from), app.cleanVPath(to)
	return app.withConn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		tables := []struct{ name, column, user string }{{"files_favorites", "path", "user_id"}, {"files_file_tags", "path", "user_id"}, {"files_versions", "path", "user_id"}, {"files_public_shares", "path", "user_id"}, {"files_comments", "path", "user_id"}, {"files_locks", "path", "user_id"}}
		for _, table := range tables {
			q := `UPDATE ` + table.name + ` SET ` + table.column + `=?||substr(` + table.column + `,?) WHERE ` + table.user + `=? AND ` + pathMatchSQL(table.column)
			args := []any{to, len(from) + 1, uid}
			args = append(args, pathArgs(from)...)
			if err = sqlitex.Execute(c, q, &sqlitex.ExecOptions{Args: args}); err != nil {
				return err
			}
		}
		if err = sqlitex.Execute(c, `UPDATE files_user_shares SET path=?||substr(path,?) WHERE from_user=? AND `+pathMatchSQL("path"), &sqlitex.ExecOptions{Args: []any{to, len(from) + 1, uid, from, from + `/%`}}); err != nil {
			return err
		}
		return nil
	})
}
func (app *App) purgePathMetadata(ctx context.Context, uid, vpath string) error {
	vpath = app.cleanVPath(vpath)
	return app.withConn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		tables := []struct{ name, column string }{{"files_favorites", "path"}, {"files_file_tags", "path"}, {"files_versions", "path"}, {"files_public_shares", "path"}, {"files_comments", "path"}, {"files_locks", "path"}}
		for _, table := range tables {
			args := []any{uid}
			args = append(args, pathArgs(vpath)...)
			if err = sqlitex.Execute(c, `DELETE FROM `+table.name+` WHERE user_id=? AND `+pathMatchSQL(table.column), &sqlitex.ExecOptions{Args: args}); err != nil {
				return err
			}
		}
		return sqlitex.Execute(c, `DELETE FROM files_user_shares WHERE from_user=? AND `+pathMatchSQL("path"), &sqlitex.ExecOptions{Args: []any{uid, vpath, vpath + `/%`}})
	})
}

func (app *App) Favorite(ctx context.Context, uid, vpath string) (bool, error) {
	var favorite bool
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT 1 FROM files_favorites WHERE user_id=? AND path=?`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath)}, ResultFunc: func(*sqlite.Stmt) error { favorite = true; return nil }})
	})
	return favorite, err
}

// Favorites returns all favorited virtual paths for one user in a single query.
func (app *App) Favorites(ctx context.Context, uid string) (map[string]bool, error) {
	out := make(map[string]bool)
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT path FROM files_favorites WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out[s.ColumnText(0)] = true
			return nil
		}})
	})
	return out, err
}

func (app *App) SetFavorite(ctx context.Context, uid, vpath string, favorite bool) error {
	return app.withConn(ctx, func(c *sqlite.Conn) error {
		q := `DELETE FROM files_favorites WHERE user_id=? AND path=?`
		if favorite {
			q = `INSERT INTO files_favorites(user_id,path) VALUES(?,?) ON CONFLICT DO NOTHING`
		}
		return sqlitex.Execute(c, q, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath)}})
	})
}

func (app *App) CreateVersion(ctx context.Context, uid, vpath string) (Version, error) {
	data, err := app.ReadFile(ctx, uid, vpath)
	if err != nil {
		return Version{}, err
	}
	id := newFileID()
	blob := filepath.Join(app.dataRoot, uid, "versions", hex.EncodeToString([]byte(app.cleanVPath(vpath))), id)
	if err := os.MkdirAll(filepath.Dir(blob), 0700); err != nil {
		return Version{}, err
	}
	tmp, err := os.CreateTemp(filepath.Dir(blob), ".dar-version-*")
	if err != nil {
		return Version{}, err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)
	if _, err = tmp.Write(data); err != nil {
		_ = tmp.Close()
		return Version{}, err
	}
	if err = tmp.Sync(); err != nil {
		_ = tmp.Close()
		return Version{}, err
	}
	if err = tmp.Close(); err != nil {
		return Version{}, err
	}
	if err = os.Chmod(tmpName, 0600); err != nil {
		return Version{}, err
	}
	if err = os.Rename(tmpName, blob); err != nil {
		return Version{}, err
	}
	v := Version{ID: id, Size: int64(len(data)), Time: time.Now().Unix()}
	err = app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO files_versions(user_id,path,id,blob_path,size,created_at) VALUES(?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath), id, blob, v.Size, v.Time}})
	})
	return v, err
}

func (app *App) Versions(ctx context.Context, uid, vpath string) ([]Version, error) {
	var out []Version
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,size,created_at FROM files_versions WHERE user_id=? AND path=? ORDER BY created_at DESC,id DESC`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath)}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Version{ID: s.ColumnText(0), Size: s.ColumnInt64(1), Time: s.ColumnInt64(2)})
			return nil
		}})
	})
	return out, err
}

func (app *App) ReadVersion(ctx context.Context, uid, vpath, id string) ([]byte, error) {
	var blob string
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT blob_path FROM files_versions WHERE user_id=? AND path=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath), id}, ResultFunc: func(s *sqlite.Stmt) error { blob = s.ColumnText(0); return nil }})
	})
	if err != nil {
		return nil, err
	}
	if blob == "" {
		return nil, os.ErrNotExist
	}
	return os.ReadFile(blob)
}

func (app *App) PruneVersions(ctx context.Context, uid, vpath string, keep int) error {
	var stale []string
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT blob_path FROM files_versions WHERE user_id=? AND path=? ORDER BY created_at DESC,id DESC LIMIT -1 OFFSET ?`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath), keep}, ResultFunc: func(s *sqlite.Stmt) error { stale = append(stale, s.ColumnText(0)); return nil }})
	})
	if err != nil {
		return err
	}
	for _, blob := range stale {
		_ = os.Remove(blob)
	}
	return app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM files_versions WHERE user_id=? AND path=? AND blob_path IN (SELECT blob_path FROM files_versions WHERE user_id=? AND path=? ORDER BY created_at DESC,id DESC LIMIT -1 OFFSET ?)`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath), uid, app.cleanVPath(vpath), keep}})
	})
}

func (app *App) Snapshot(ctx context.Context, uid, vpath string) error {
	_, err := app.CreateVersion(ctx, uid, vpath)
	return err
}
func (app *App) VersionIDs(ctx context.Context, uid, vpath string) ([]string, error) {
	versions, err := app.Versions(ctx, uid, vpath)
	ids := make([]string, 0, len(versions))
	for _, version := range versions {
		ids = append(ids, version.ID)
	}
	return ids, err
}
func (app *App) ReadSnapshot(ctx context.Context, uid, vpath, id string) ([]byte, error) {
	return app.ReadVersion(ctx, uid, vpath, id)
}
func (app *App) PruneSnapshots(ctx context.Context, uid, vpath string, keep int) error {
	return app.PruneVersions(ctx, uid, vpath, keep)
}

func (app *App) writeMutationError(w http.ResponseWriter, err error) {
	if errors.Is(err, ErrManagedPathReadOnly) {
		http.Error(w, "managed Maildir is read-only in Files; use Email to change messages", http.StatusForbidden)
		return
	}
	if errors.Is(err, ErrShareReadOnly) || errors.Is(err, ErrSharedRootMutation) {
		http.Error(w, err.Error(), http.StatusForbidden)
		return
	}
	if errors.Is(err, ErrLocked) {
		http.Error(w, "locked", http.StatusLocked)
		return
	}
	http.Error(w, "file operation failed", http.StatusInternalServerError)
}
