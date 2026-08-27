package notes

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"mime"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS notes_index (
		id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, path TEXT NOT NULL,
		device INTEGER NOT NULL DEFAULT 0, inode INTEGER NOT NULL DEFAULT 0, fingerprint TEXT NOT NULL,
		title TEXT NOT NULL, category TEXT NOT NULL, content TEXT NOT NULL, etag TEXT NOT NULL,
		modified INTEGER NOT NULL, reconciled_at INTEGER NOT NULL, tombstone INTEGER NOT NULL DEFAULT 0,
		last_version_at INTEGER NOT NULL DEFAULT 0, UNIQUE(user_id,path)
	);`,
	`CREATE INDEX IF NOT EXISTS idx_notes_identity ON notes_index(user_id,device,inode);`,
	`CREATE INDEX IF NOT EXISTS idx_notes_fingerprint ON notes_index(user_id,fingerprint);`,
	`CREATE INDEX IF NOT EXISTS idx_notes_search ON notes_index(user_id,tombstone,modified DESC);`,
	`CREATE TABLE IF NOT EXISTS notes_undo (user_id TEXT NOT NULL, note_id INTEGER NOT NULL, path TEXT NOT NULL, content TEXT NOT NULL, favorite INTEGER NOT NULL, deleted_at INTEGER NOT NULL, PRIMARY KEY(user_id,note_id));`,
}

func Migrations() []string { return migrations }

// FileStore is owned by Notes; Files satisfies it without Notes depending on a concrete app.
type FileStore interface {
	ReadFile(context.Context, string, string) ([]byte, error)
	Stat(context.Context, string, string) (os.FileInfo, error)
	Walk(context.Context, string, string, filepath.WalkFunc) error
	MkdirAll(context.Context, string, string) error
	WriteFile(context.Context, string, string, []byte) error
	Rename(context.Context, string, string, string) error
	Remove(context.Context, string, string) error
	Favorite(context.Context, string, string) (bool, error)
	Favorites(context.Context, string) (map[string]bool, error)
	SetFavorite(context.Context, string, string, bool) error
	Snapshot(context.Context, string, string) error
	VersionIDs(context.Context, string, string) ([]string, error)
	ReadSnapshot(context.Context, string, string, string) ([]byte, error)
	PruneSnapshots(context.Context, string, string, int) error
	SetModTime(context.Context, string, string, time.Time) error
}

const (
	scanFreshFor = 10 * time.Second
	scanKeepFor  = time.Minute
)

type userScanState struct {
	running     bool
	done        chan struct{}
	lastScan    time.Time
	completedAt time.Time
	lastErr     error
}

type scanCoordinator struct {
	mu    sync.Mutex
	users map[string]*userScanState
}

func newScanCoordinator() scanCoordinator {
	return scanCoordinator{users: make(map[string]*userScanState)}
}

func (a *App) invalidateScan(uid string) {
	a.scans.mu.Lock()
	if state := a.scans.users[uid]; state != nil {
		state.lastScan = time.Time{}
	}
	a.scans.mu.Unlock()
}

func (a *App) ensureFresh(ctx context.Context, uid string) error {
	for {
		now := time.Now()
		a.scans.mu.Lock()
		for key, state := range a.scans.users {
			if !state.running && !state.completedAt.IsZero() && now.Sub(state.completedAt) > scanKeepFor {
				delete(a.scans.users, key)
			}
		}
		state := a.scans.users[uid]
		if state == nil {
			state = &userScanState{}
			a.scans.users[uid] = state
		}
		if state.running {
			done := state.done
			a.scans.mu.Unlock()
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-done:
				a.scans.mu.Lock()
				err := state.lastErr
				a.scans.mu.Unlock()
				return err
			}
		}
		if !state.lastScan.IsZero() && now.Sub(state.lastScan) < scanFreshFor {
			err := state.lastErr
			a.scans.mu.Unlock()
			return err
		}
		state.running = true
		state.done = make(chan struct{})
		a.scans.mu.Unlock()

		err := a.reconcile(ctx, uid)

		a.scans.mu.Lock()
		state.running = false
		state.lastErr = err
		state.completedAt = time.Now()
		if err == nil {
			state.lastScan = state.completedAt
		} else {
			state.lastScan = time.Time{}
		}
		close(state.done)
		a.scans.mu.Unlock()
		return err
	}
}

type Note struct {
	ID            int64  `json:"id"`
	ETag          string `json:"etag"`
	Readonly      bool   `json:"readonly"`
	Content       string `json:"content"`
	Title         string `json:"title"`
	Category      string `json:"category"`
	Favorite      bool   `json:"favorite"`
	Modified      int64  `json:"modified"`
	Path          string `json:"-"`
	LastVersionAt int64  `json:"-"`
	Tombstone     bool   `json:"-"`
}

type Settings struct {
	NotesPath  string `json:"notesPath"`
	FileSuffix string `json:"fileSuffix"`
	EditorMode string `json:"editorMode,omitempty"`
}

func defaultSettings() Settings {
	return Settings{NotesPath: "/Notes", FileSuffix: ".md", EditorMode: "rich"}
}

func (a *App) conn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return errors.New("notes database is not configured")
	}
	c, release, err := a.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func (a *App) settings(ctx context.Context, uid string) (Settings, error) {
	out := defaultSettings()
	err := a.conn(ctx, func(c *sqlite.Conn) error {
		_, err := preferences.GetJSON(c, uid, "notes", "settings", &out)
		return err
	})
	return sanitizeSettings(out), err
}
func sanitizeSettings(s Settings) Settings {
	d := defaultSettings()
	s.NotesPath = path.Clean("/" + strings.TrimPrefix(strings.TrimSpace(s.NotesPath), "/"))
	if s.NotesPath == "/" || strings.Contains(s.NotesPath, "..") {
		s.NotesPath = d.NotesPath
	}
	s.FileSuffix = "." + strings.TrimLeft(strings.TrimSpace(s.FileSuffix), ".")
	if s.FileSuffix == "." || strings.ContainsAny(s.FileSuffix, "/\\\x00") || len(s.FileSuffix) > 16 {
		s.FileSuffix = d.FileSuffix
	}
	switch s.EditorMode {
	case "rich", "source", "preview":
	default:
		s.EditorMode = d.EditorMode
	}
	return s
}
func (a *App) saveSettings(ctx context.Context, uid string, s Settings) error {
	s = sanitizeSettings(s)
	return a.conn(ctx, func(c *sqlite.Conn) error { return preferences.SetJSON(c, uid, "notes", "settings", s) })
}

func fingerprint(data []byte) string { h := sha256.Sum256(data); return hex.EncodeToString(h[:]) }
func etag(data []byte, modified int64, vpath string, favorite bool) string {
	seed := append(append([]byte{}, data...), []byte(strconv.FormatInt(modified, 10)+"\x00"+vpath+"\x00"+strconv.FormatBool(favorite))...)
	h := sha256.Sum256(seed)
	return hex.EncodeToString(h[:16])
}
func identity(info os.FileInfo) (uint64, uint64) {
	if s, ok := info.Sys().(*syscall.Stat_t); ok {
		return s.Dev, s.Ino
	}
	return 0, 0
}
func titleFrom(pathName, content, suffix string) string {
	return strings.TrimSuffix(filepath.Base(pathName), suffix)
}
func categoryFor(root, vpath string) string {
	rel := strings.TrimPrefix(strings.TrimPrefix(vpath, root), "/")
	dir := path.Dir(rel)
	if dir == "." {
		return ""
	}
	return dir
}

type diskNote struct {
	path, content, title, category, fp, tag string
	modified                                int64
	dev, ino                                uint64
	favorite                                bool
}

func (a *App) reconcile(ctx context.Context, uid string) error {
	s, err := a.settings(ctx, uid)
	if err != nil {
		return err
	}
	if err = a.files.MkdirAll(ctx, uid, s.NotesPath); err != nil {
		return err
	}
	favorites, err := a.files.Favorites(ctx, uid)
	if err != nil {
		return err
	}
	var disk []diskNote
	err = a.files.Walk(ctx, uid, s.NotesPath, func(vpath string, info os.FileInfo, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if info.IsDir() || !strings.HasSuffix(strings.ToLower(info.Name()), strings.ToLower(s.FileSuffix)) {
			return nil
		}
		data, readErr := a.files.ReadFile(ctx, uid, vpath)
		if readErr != nil {
			return readErr
		}
		dev, ino := identity(info)
		favorite := favorites[vpath]
		disk = append(disk, diskNote{path: vpath, content: string(data), title: titleFrom(vpath, string(data), s.FileSuffix), category: categoryFor(s.NotesPath, vpath), fp: fingerprint(data), tag: etag(data, info.ModTime().Unix(), vpath, favorite), modified: info.ModTime().Unix(), dev: dev, ino: ino, favorite: favorite})
		return nil
	})
	if err != nil {
		return err
	}
	return a.conn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		type existingNote struct {
			id        int64
			path, fp  string
			tag       string
			dev, ino  uint64
			tombstone bool
		}
		var existing []existingNote
		if err = sqlitex.Execute(c, `SELECT id,path,fingerprint,device,inode,etag,tombstone FROM notes_index WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(st *sqlite.Stmt) error {
			existing = append(existing, existingNote{id: st.ColumnInt64(0), path: st.ColumnText(1), fp: st.ColumnText(2), dev: uint64(st.ColumnInt64(3)), ino: uint64(st.ColumnInt64(4)), tag: st.ColumnText(5), tombstone: st.ColumnInt64(6) != 0})
			return nil
		}}); err != nil {
			return err
		}
		type fileIdentity struct{ dev, ino uint64 }
		byPath := make(map[string]int, len(existing))
		byIdentity := make(map[fileIdentity][]int, len(existing))
		byFingerprint := make(map[string][]int, len(existing))
		for i, note := range existing {
			byPath[note.path] = i
			if note.dev != 0 && note.ino != 0 {
				key := fileIdentity{note.dev, note.ino}
				byIdentity[key] = append(byIdentity[key], i)
			}
			byFingerprint[note.fp] = append(byFingerprint[note.fp], i)
		}
		used := map[int64]bool{}
		now := time.Now().Unix()
		for _, d := range disk {
			var id int64
			matchedIndex := -1
			if i, ok := byPath[d.path]; ok && !used[existing[i].id] {
				id = existing[i].id
				matchedIndex = i
			}
			if id == 0 && d.dev != 0 && d.ino != 0 {
				matches := byIdentity[fileIdentity{d.dev, d.ino}]
				if len(matches) == 1 && !used[existing[matches[0]].id] {
					id = existing[matches[0]].id
					matchedIndex = matches[0]
				}
			}
			if id == 0 {
				matches := byFingerprint[d.fp]
				if len(matches) == 1 && !used[existing[matches[0]].id] {
					id = existing[matches[0]].id
					matchedIndex = matches[0]
				}
			}
			if id == 0 {
				err = sqlitex.Execute(c, `INSERT INTO notes_index(user_id,path,device,inode,fingerprint,title,category,content,etag,modified,reconciled_at,tombstone) VALUES(?,?,?,?,?,?,?,?,?,?,?,0)`, &sqlitex.ExecOptions{Args: []any{uid, d.path, int64(d.dev), int64(d.ino), d.fp, d.title, d.category, d.content, d.tag, d.modified, now}})
				if err != nil {
					return err
				}
				id = c.LastInsertRowID()
			} else if current := existing[matchedIndex]; current.path != d.path || current.fp != d.fp || current.dev != d.dev || current.ino != d.ino || current.tag != d.tag || current.tombstone {
				err = sqlitex.Execute(c, `UPDATE notes_index SET path=?,device=?,inode=?,fingerprint=?,title=?,category=?,content=?,etag=?,modified=?,reconciled_at=?,tombstone=0 WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{d.path, int64(d.dev), int64(d.ino), d.fp, d.title, d.category, d.content, d.tag, d.modified, now, uid, id}})
				if err != nil {
					return err
				}
			}
			used[id] = true
		}
		for _, e := range existing {
			if !used[e.id] {
				if err = sqlitex.Execute(c, `UPDATE notes_index SET tombstone=1,reconciled_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{now, uid, e.id}}); err != nil {
					return err
				}
			}
		}
		return nil
	})
}

func (a *App) indexedNotes(ctx context.Context, uid string, includeTombstone bool) ([]Note, error) {
	return a.searchIndexedNotes(ctx, uid, includeTombstone, "")
}

func (a *App) searchIndexedNotes(ctx context.Context, uid string, includeTombstone bool, query string) ([]Note, error) {
	if err := a.ensureFresh(ctx, uid); err != nil {
		return nil, err
	}
	var out []Note
	where := "tombstone=0"
	if includeTombstone {
		where = "1=1"
	}
	args := []any{uid}
	if query = strings.ToLower(strings.TrimSpace(query)); query != "" {
		where += ` AND (instr(lower(title),?)>0 OR instr(lower(category),?)>0 OR instr(lower(content),?)>0)`
		args = append(args, query, query, query)
	}
	err := a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,etag,content,title,category,modified,path,last_version_at,tombstone FROM notes_index WHERE user_id=? AND `+where+` ORDER BY modified DESC,id DESC`, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error {
			n := Note{ID: s.ColumnInt64(0), ETag: s.ColumnText(1), Content: s.ColumnText(2), Title: s.ColumnText(3), Category: s.ColumnText(4), Modified: s.ColumnInt64(5), Path: s.ColumnText(6), LastVersionAt: s.ColumnInt64(7), Tombstone: s.ColumnInt64(8) != 0}
			out = append(out, n)
			return nil
		}})
	})
	if err != nil {
		return nil, err
	}
	favorites, err := a.files.Favorites(ctx, uid)
	if err != nil {
		return nil, err
	}
	for i := range out {
		out[i].Favorite = favorites[out[i].Path]
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Favorite != out[j].Favorite {
			return out[i].Favorite
		}
		return out[i].Modified > out[j].Modified
	})
	return out, nil
}

func (a *App) loadIndexedNote(ctx context.Context, uid string, id int64) (Note, error) {
	var out Note
	err := a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,etag,content,title,category,modified,path,last_version_at,tombstone FROM notes_index WHERE user_id=? AND id=? AND tombstone=0`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error {
			out = Note{ID: s.ColumnInt64(0), ETag: s.ColumnText(1), Content: s.ColumnText(2), Title: s.ColumnText(3), Category: s.ColumnText(4), Modified: s.ColumnInt64(5), Path: s.ColumnText(6), LastVersionAt: s.ColumnInt64(7), Tombstone: s.ColumnInt64(8) != 0}
			return nil
		}})
	})
	if err != nil {
		return Note{}, err
	}
	if out.ID == 0 {
		return Note{}, os.ErrNotExist
	}
	return out, nil
}

func (a *App) note(ctx context.Context, uid string, id int64) (Note, error) {
	if err := a.ensureFresh(ctx, uid); err != nil {
		return Note{}, err
	}
	out, err := a.loadIndexedNote(ctx, uid, id)
	if err != nil {
		return Note{}, err
	}
	out.Favorite, err = a.files.Favorite(ctx, uid, out.Path)
	return out, err
}

func safePart(s string) string {
	s = strings.TrimSpace(s)
	s = strings.Map(func(r rune) rune {
		if r < ' ' || strings.ContainsRune(`/\\:*?"<>|`, r) {
			return -1
		}
		return r
	}, s)
	return strings.Trim(s, ". ")
}
func (a *App) uniquePath(ctx context.Context, uid string, s Settings, title, category string, except int64) (string, string, error) {
	title = safePart(title)
	if title == "" {
		title = "New note"
	}
	parts := strings.Split(strings.Trim(category, "/"), "/")
	for i := range parts {
		parts[i] = safePart(parts[i])
	}
	category = strings.Join(parts, "/")
	dir := s.NotesPath
	if category != "" {
		dir = path.Join(dir, category)
	}
	if err := a.files.MkdirAll(ctx, uid, dir); err != nil {
		return "", "", err
	}
	for i := 0; ; i++ {
		name := title
		if i > 0 {
			name += fmt.Sprintf(" (%d)", i+1)
		}
		candidate := path.Join(dir, name+s.FileSuffix)
		var owner int64
		_ = a.conn(ctx, func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `SELECT id FROM notes_index WHERE user_id=? AND path=? AND tombstone=0`, &sqlitex.ExecOptions{Args: []any{uid, candidate}, ResultFunc: func(st *sqlite.Stmt) error { owner = st.ColumnInt64(0); return nil }})
		})
		if owner == 0 || owner == except {
			return candidate, title, nil
		}
	}
}

type notePatch struct {
	Content  *string `json:"content"`
	Title    *string `json:"title"`
	Category *string `json:"category"`
	Favorite *bool   `json:"favorite"`
	Modified *int64  `json:"modified"`
}

func (a *App) noteFromFile(ctx context.Context, uid string, s Settings, id int64, vpath, title, content string, favorite bool, lastVersionAt int64) (Note, uint64, uint64, error) {
	info, err := a.files.Stat(ctx, uid, vpath)
	if err != nil {
		return Note{}, 0, 0, err
	}
	dev, ino := identity(info)
	modified := info.ModTime().Unix()
	n := Note{ID: id, ETag: etag([]byte(content), modified, vpath, favorite), Content: content, Title: title, Category: categoryFor(s.NotesPath, vpath), Favorite: favorite, Modified: modified, Path: vpath, LastVersionAt: lastVersionAt}
	return n, dev, ino, nil
}

func (a *App) insertIndexedNote(ctx context.Context, uid string, n *Note, dev, ino uint64) error {
	return a.conn(ctx, func(c *sqlite.Conn) error {
		err := sqlitex.Execute(c, `INSERT INTO notes_index(user_id,path,device,inode,fingerprint,title,category,content,etag,modified,reconciled_at,tombstone,last_version_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,0,?)`, &sqlitex.ExecOptions{Args: []any{uid, n.Path, int64(dev), int64(ino), fingerprint([]byte(n.Content)), n.Title, n.Category, n.Content, n.ETag, n.Modified, time.Now().Unix(), n.LastVersionAt}})
		if err == nil {
			n.ID = c.LastInsertRowID()
		}
		return err
	})
}

func (a *App) updateIndexedNote(ctx context.Context, uid string, n Note, dev, ino uint64) error {
	return a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE notes_index SET path=?,device=?,inode=?,fingerprint=?,title=?,category=?,content=?,etag=?,modified=?,reconciled_at=?,tombstone=0,last_version_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{n.Path, int64(dev), int64(ino), fingerprint([]byte(n.Content)), n.Title, n.Category, n.Content, n.ETag, n.Modified, time.Now().Unix(), n.LastVersionAt, uid, n.ID}})
	})
}

func (a *App) create(ctx context.Context, uid string, p notePatch) (Note, error) {
	defer a.markSearchDirty()
	if err := a.ensureFresh(ctx, uid); err != nil {
		return Note{}, err
	}
	s, err := a.settings(ctx, uid)
	if err != nil {
		return Note{}, err
	}
	title := "New note"
	category, content := "", ""
	favorite := false
	if p.Title != nil {
		title = *p.Title
	}
	if p.Category != nil {
		category = *p.Category
	}
	if p.Content != nil {
		content = *p.Content
	}
	if p.Favorite != nil {
		favorite = *p.Favorite
	}
	vpath, title, err := a.uniquePath(ctx, uid, s, title, category, 0)
	if err != nil {
		return Note{}, err
	}
	if err = a.files.WriteFile(ctx, uid, vpath, []byte(content)); err != nil {
		return Note{}, err
	}
	written := true
	if p.Modified != nil && *p.Modified > 0 {
		if err = a.files.SetModTime(ctx, uid, vpath, time.Unix(*p.Modified, 0)); err != nil {
			a.invalidateScan(uid)
			return Note{}, err
		}
	}
	if err = a.files.SetFavorite(ctx, uid, vpath, favorite); err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	n, dev, ino, err := a.noteFromFile(ctx, uid, s, 0, vpath, title, content, favorite, 0)
	if err != nil {
		if written {
			a.invalidateScan(uid)
		}
		return Note{}, err
	}
	if err = a.insertIndexedNote(ctx, uid, &n, dev, ino); err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	a.replaceNoteSearch(ctx, uid, n)
	return n, nil
}

func (a *App) update(ctx context.Context, uid string, id int64, p notePatch, ifMatch string, force bool) (Note, error) {
	defer a.markSearchDirty()
	if err := a.ensureFresh(ctx, uid); err != nil {
		return Note{}, err
	}
	current, err := a.loadIndexedNote(ctx, uid, id)
	if err != nil {
		return Note{}, err
	}
	current.Favorite, err = a.files.Favorite(ctx, uid, current.Path)
	if err != nil {
		return Note{}, err
	}
	if !force && ifMatch != "" && strings.Trim(ifMatch, "\"") != current.ETag {
		return current, errConflict
	}
	title, category, content, favorite := current.Title, current.Category, current.Content, current.Favorite
	if p.Title != nil {
		title = *p.Title
	}
	if p.Category != nil {
		category = *p.Category
	}
	if p.Content != nil {
		content = *p.Content
	}
	if p.Favorite != nil {
		favorite = *p.Favorite
	}
	s, err := a.settings(ctx, uid)
	if err != nil {
		return Note{}, err
	}
	target, title, err := a.uniquePath(ctx, uid, s, title, category, id)
	if err != nil {
		return Note{}, err
	}
	if current.LastVersionAt == 0 || time.Now().Unix()-current.LastVersionAt >= 300 {
		if e := a.files.Snapshot(ctx, uid, current.Path); e == nil {
			_ = a.files.PruneSnapshots(ctx, uid, current.Path, 50)
			current.LastVersionAt = time.Now().Unix()
		}
	}
	if target != current.Path {
		if err = a.files.Rename(ctx, uid, current.Path, target); err != nil {
			return Note{}, err
		}
		a.invalidateScan(uid)
	}
	if content != current.Content {
		if err = a.files.WriteFile(ctx, uid, target, []byte(content)); err != nil {
			a.invalidateScan(uid)
			return Note{}, err
		}
	}
	if p.Modified != nil && *p.Modified > 0 {
		if err = a.files.SetModTime(ctx, uid, target, time.Unix(*p.Modified, 0)); err != nil {
			a.invalidateScan(uid)
			return Note{}, err
		}
	}
	if favorite != current.Favorite {
		if err = a.files.SetFavorite(ctx, uid, target, favorite); err != nil {
			a.invalidateScan(uid)
			return Note{}, err
		}
	}
	out, dev, ino, err := a.noteFromFile(ctx, uid, s, id, target, title, content, favorite, current.LastVersionAt)
	if err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	if err = a.updateIndexedNote(ctx, uid, out, dev, ino); err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	a.replaceNoteSearch(ctx, uid, out)
	return out, nil
}

var errConflict = errors.New("note conflict")
var errCategoryNotEmpty = errors.New("category is not empty")

func (a *App) renameCategory(ctx context.Context, uid, from, to string) error {
	defer a.markSearchDirty()
	if err := a.ensureFresh(ctx, uid); err != nil {
		return err
	}
	s, err := a.settings(ctx, uid)
	if err != nil {
		return err
	}
	oldRoot := path.Join(s.NotesPath, strings.Trim(from, "/"))
	newRoot := path.Join(s.NotesPath, strings.Trim(to, "/"))
	if err = a.files.Rename(ctx, uid, oldRoot, newRoot); err != nil {
		return err
	}
	var affected []Note
	err = a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,etag,content,title,category,modified,path,last_version_at,tombstone FROM notes_index WHERE user_id=? AND tombstone=0`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(st *sqlite.Stmt) error {
			n := Note{ID: st.ColumnInt64(0), ETag: st.ColumnText(1), Content: st.ColumnText(2), Title: st.ColumnText(3), Category: st.ColumnText(4), Modified: st.ColumnInt64(5), Path: st.ColumnText(6), LastVersionAt: st.ColumnInt64(7), Tombstone: st.ColumnInt64(8) != 0}
			if n.Path == oldRoot || strings.HasPrefix(n.Path, oldRoot+"/") {
				affected = append(affected, n)
			}
			return nil
		}})
	})
	if err != nil {
		a.invalidateScan(uid)
		return err
	}
	favorites, err := a.files.Favorites(ctx, uid)
	if err != nil {
		a.invalidateScan(uid)
		return err
	}
	for i := range affected {
		affected[i].Path = newRoot + strings.TrimPrefix(affected[i].Path, oldRoot)
		affected[i].Category = categoryFor(s.NotesPath, affected[i].Path)
		affected[i].Favorite = favorites[affected[i].Path]
		affected[i].ETag = etag([]byte(affected[i].Content), affected[i].Modified, affected[i].Path, affected[i].Favorite)
	}
	err = a.conn(ctx, func(c *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(c)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		for _, n := range affected {
			if txErr = sqlitex.Execute(c, `UPDATE notes_index SET path=?,category=?,etag=?,reconciled_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{n.Path, n.Category, n.ETag, time.Now().Unix(), uid, n.ID}}); txErr != nil {
				return txErr
			}
		}
		return nil
	})
	if err != nil {
		a.invalidateScan(uid)
	} else {
		for _, note := range affected {
			a.replaceNoteSearch(ctx, uid, note)
		}
	}
	return err
}

func (a *App) deleteCategory(ctx context.Context, uid, name string, force bool) error {
	defer a.markSearchDirty()
	if err := a.ensureFresh(ctx, uid); err != nil {
		return err
	}
	var ids []int64
	err := a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,category FROM notes_index WHERE user_id=? AND tombstone=0`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(st *sqlite.Stmt) error {
			category := st.ColumnText(1)
			if category == name || strings.HasPrefix(category, name+"/") {
				ids = append(ids, st.ColumnInt64(0))
			}
			return nil
		}})
	})
	if err != nil {
		return err
	}
	if len(ids) > 0 && !force {
		return errCategoryNotEmpty
	}
	s, err := a.settings(ctx, uid)
	if err != nil {
		return err
	}
	if err = a.files.Remove(ctx, uid, path.Join(s.NotesPath, name)); err != nil {
		return err
	}
	err = a.conn(ctx, func(c *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(c)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		for _, id := range ids {
			if txErr = sqlitex.Execute(c, `UPDATE notes_index SET tombstone=1,reconciled_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{time.Now().Unix(), uid, id}}); txErr != nil {
				return txErr
			}
		}
		return nil
	})
	if err != nil {
		a.invalidateScan(uid)
	} else {
		for _, noteID := range ids {
			a.deleteNoteSearch(ctx, uid, noteID)
		}
	}
	return err
}

func (a *App) delete(ctx context.Context, uid string, id int64) error {
	defer a.markSearchDirty()
	if err := a.ensureFresh(ctx, uid); err != nil {
		return err
	}
	n, err := a.loadIndexedNote(ctx, uid, id)
	if err != nil {
		return err
	}
	fav, _ := a.files.Favorite(ctx, uid, n.Path)
	if err = a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO notes_undo(user_id,note_id,path,content,favorite,deleted_at) VALUES(?,?,?,?,?,?) ON CONFLICT(user_id,note_id) DO UPDATE SET path=excluded.path,content=excluded.content,favorite=excluded.favorite,deleted_at=excluded.deleted_at`, &sqlitex.ExecOptions{Args: []any{uid, id, n.Path, n.Content, boolInt(fav), time.Now().Unix()}})
	}); err != nil {
		return err
	}
	if err = a.files.Remove(ctx, uid, n.Path); err != nil {
		return err
	}
	err = a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE notes_index SET tombstone=1 WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
	})
	if err != nil {
		a.invalidateScan(uid)
	} else {
		a.deleteNoteSearch(ctx, uid, id)
	}
	return err
}
func (a *App) undo(ctx context.Context, uid string, id int64) (Note, error) {
	defer a.markSearchDirty()
	var vpath, content string
	fav := false
	err := a.conn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT path,content,favorite FROM notes_undo WHERE user_id=? AND note_id=? AND deleted_at>?`, &sqlitex.ExecOptions{Args: []any{uid, id, time.Now().Add(-30 * time.Second).Unix()}, ResultFunc: func(s *sqlite.Stmt) error {
			vpath = s.ColumnText(0)
			content = s.ColumnText(1)
			fav = s.ColumnInt64(2) != 0
			return nil
		}})
	})
	if err != nil {
		return Note{}, err
	}
	if vpath == "" {
		return Note{}, os.ErrNotExist
	}
	if err = a.files.WriteFile(ctx, uid, vpath, []byte(content)); err != nil {
		return Note{}, err
	}
	if err = a.files.SetFavorite(ctx, uid, vpath, fav); err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	s, err := a.settings(ctx, uid)
	if err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	title := titleFrom(vpath, content, s.FileSuffix)
	n, dev, ino, err := a.noteFromFile(ctx, uid, s, id, vpath, title, content, fav, 0)
	if err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	err = a.conn(ctx, func(c *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(c)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		if txErr = sqlitex.Execute(c, `UPDATE notes_index SET path=?,device=?,inode=?,fingerprint=?,title=?,category=?,content=?,etag=?,modified=?,reconciled_at=?,tombstone=0,last_version_at=0 WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{n.Path, int64(dev), int64(ino), fingerprint([]byte(n.Content)), n.Title, n.Category, n.Content, n.ETag, n.Modified, time.Now().Unix(), uid, id}}); txErr != nil {
			return txErr
		}
		if c.Changes() == 0 {
			if txErr = sqlitex.Execute(c, `INSERT INTO notes_index(id,user_id,path,device,inode,fingerprint,title,category,content,etag,modified,reconciled_at,tombstone,last_version_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,0,0)`, &sqlitex.ExecOptions{Args: []any{id, uid, n.Path, int64(dev), int64(ino), fingerprint([]byte(n.Content)), n.Title, n.Category, n.Content, n.ETag, n.Modified, time.Now().Unix()}}); txErr != nil {
				return txErr
			}
		}
		return sqlitex.Execute(c, `DELETE FROM notes_undo WHERE user_id=? AND note_id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
	})
	if err != nil {
		a.invalidateScan(uid)
		return Note{}, err
	}
	a.replaceNoteSearch(ctx, uid, n)
	return n, nil
}

func randomName(ext string) string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b) + ext
}
func validImageType(header []byte, name string) (string, string, bool) {
	det := http.DetectContentType(header)
	allowed := map[string]string{"image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp"}
	ext, ok := allowed[det]
	if !ok {
		return "", "", false
	}
	if named := strings.ToLower(filepath.Ext(name)); named != "" && mime.TypeByExtension(named) != "" && !strings.HasPrefix(mime.TypeByExtension(named), "image/") {
		return "", "", false
	}
	return det, ext, true
}

func (a *App) userID(r *http.Request) string {
	if u := server.GetUser(r); u != nil {
		return u.ID
	}
	return ""
}
func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
func parseID(raw string) (int64, error) {
	id, err := strconv.ParseInt(raw, 10, 64)
	if err != nil || id <= 0 {
		return 0, errors.New("invalid id")
	}
	return id, nil
}
