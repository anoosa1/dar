package news

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS news_folders (
		id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, name TEXT NOT NULL,
		created_at INTEGER NOT NULL DEFAULT 0, UNIQUE(user_id, name)
	);`,
	`CREATE TABLE IF NOT EXISTS news_feeds (
		id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, folder_id INTEGER,
		url TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', link TEXT NOT NULL DEFAULT '', favicon_link TEXT NOT NULL DEFAULT '',
		added INTEGER NOT NULL DEFAULT 0, last_update INTEGER NOT NULL DEFAULT 0, next_update INTEGER NOT NULL DEFAULT 0,
		articles_per_update INTEGER NOT NULL DEFAULT 0, update_error_count INTEGER NOT NULL DEFAULT 0, last_update_error TEXT NOT NULL DEFAULT '',
		ordering INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0, update_enabled INTEGER NOT NULL DEFAULT 1,
		update_mode INTEGER NOT NULL DEFAULT 0, full_text_enabled INTEGER NOT NULL DEFAULT 0, keyword_filters TEXT NOT NULL DEFAULT '',
		credential_key_id TEXT NOT NULL DEFAULT '', credential_nonce BLOB NOT NULL DEFAULT X'', credential_cipher BLOB NOT NULL DEFAULT X'',
		UNIQUE(user_id, url), FOREIGN KEY(folder_id) REFERENCES news_folders(id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_news_feeds_user_folder ON news_feeds(user_id, folder_id, pinned DESC, title);`,
	`CREATE TABLE IF NOT EXISTS news_items (
		id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, feed_id INTEGER NOT NULL,
		guid TEXT NOT NULL, guid_hash TEXT NOT NULL, url TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', author TEXT,
		pub_date INTEGER NOT NULL DEFAULT 0, body TEXT NOT NULL DEFAULT '', enclosure_mime TEXT, enclosure_link TEXT,
		media_thumbnail TEXT, media_description TEXT, unread INTEGER NOT NULL DEFAULT 1, starred INTEGER NOT NULL DEFAULT 0,
		rtl INTEGER NOT NULL DEFAULT 0, last_modified INTEGER NOT NULL DEFAULT 0, fingerprint TEXT NOT NULL DEFAULT '',
		viewed_at INTEGER NOT NULL DEFAULT 0, UNIQUE(user_id, feed_id, guid_hash),
		FOREIGN KEY(feed_id) REFERENCES news_feeds(id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_news_items_query ON news_items(user_id, feed_id, unread, starred, id DESC);`,
	`CREATE INDEX IF NOT EXISTS idx_news_items_modified ON news_items(user_id, last_modified);`,
}

func Migrations() []string { return migrations }

func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func (a *App) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return fmt.Errorf("news database is not configured")
	}
	c, release, err := a.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}

func (a *App) folders(ctx context.Context, uid string) ([]Folder, error) {
	var out []Folder
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,name FROM news_folders WHERE user_id=? ORDER BY lower(name),id`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Folder{ID: s.ColumnInt64(0), Name: s.ColumnText(1)})
			return nil
		}})
	})
	return out, err
}

func (a *App) createFolder(ctx context.Context, uid, name string) (Folder, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		return Folder{}, errInvalid
	}
	var out Folder
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_folders(user_id,name,created_at) VALUES(?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, name, time.Now().Unix()}}); err != nil {
			return err
		}
		out = Folder{ID: c.LastInsertRowID(), Name: name}
		return nil
	})
	return out, err
}

func (a *App) renameFolder(ctx context.Context, uid string, id int64, name string) error {
	name = strings.TrimSpace(name)
	if name == "" {
		return errInvalid
	}
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return execOwned(c, `UPDATE news_folders SET name=? WHERE user_id=? AND id=?`, []any{name, uid, id})
	})
}

func (a *App) deleteFolder(ctx context.Context, uid string, id int64) error {
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		var err error
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM news_items WHERE user_id=? AND feed_id IN (SELECT id FROM news_feeds WHERE user_id=? AND folder_id=?)`, &sqlitex.ExecOptions{Args: []any{uid, uid, id}}); err != nil {
			return err
		}
		if err = sqlitex.Execute(c, `DELETE FROM news_feeds WHERE user_id=? AND folder_id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}}); err != nil {
			return err
		}
		err = execOwned(c, `DELETE FROM news_folders WHERE user_id=? AND id=?`, []any{uid, id})
		return err
	})
}

func execOwned(c *sqlite.Conn, q string, args []any) error {
	if err := sqlitex.Execute(c, q, &sqlitex.ExecOptions{Args: args}); err != nil {
		return err
	}
	if c.Changes() == 0 {
		return errNotFound
	}
	return nil
}

func (a *App) scanFeed(uid string, s *sqlite.Stmt) (Feed, error) {
	f := Feed{ID: s.ColumnInt64(0), URL: s.ColumnText(1), Title: s.ColumnText(2), FaviconLink: s.ColumnText(3), Added: s.ColumnInt64(4), NextUpdateTime: s.ColumnInt64(5), FolderID: s.ColumnInt64(6), UnreadCount: s.ColumnInt64(7), Ordering: int(s.ColumnInt64(8)), Link: s.ColumnText(9), Pinned: s.ColumnInt64(10) != 0, UpdateErrorCount: int(s.ColumnInt64(11)), LastUpdateError: s.ColumnText(12), LastUpdateTime: s.ColumnInt64(13), ArticlesPerUpdate: int(s.ColumnInt64(14)), UpdateEnabled: s.ColumnInt64(15) != 0, UpdateMode: int(s.ColumnInt64(16)), FullTextEnabled: s.ColumnInt64(17) != 0, KeywordFilters: s.ColumnText(18), credentialKeyID: s.ColumnText(19), credentialNonce: columnBlob(s, 20), credentialCipher: columnBlob(s, 21)}
	if f.credentialKeyID == "" {
		return f, nil
	}
	if a.keyErr != nil {
		return Feed{}, a.keyErr
	}
	plain, _, err := a.keys.Decrypt(f.credentialKeyID, f.credentialNonce, f.credentialCipher, newsCredentialAAD(uid, f.ID))
	if err != nil {
		return Feed{}, err
	}
	var values []string
	if err := json.Unmarshal([]byte(plain), &values); err != nil || len(values) != 2 {
		return Feed{}, fmt.Errorf("decode feed credentials: invalid payload")
	}
	f.BasicAuthUser, f.BasicAuthPassword = values[0], values[1]
	return f, nil
}

func columnBlob(s *sqlite.Stmt, column int) []byte {
	b := make([]byte, s.ColumnLen(column))
	s.ColumnBytes(column, b)
	return b
}

func newsCredentialAAD(uid string, id int64) []byte {
	return []byte(fmt.Sprintf("dar-news-v1\x00%s\x00%d", uid, id))
}

const feedSelect = `SELECT f.id,f.url,f.title,f.favicon_link,f.added,f.next_update,COALESCE(f.folder_id,0),(SELECT count(*) FROM news_items i WHERE i.user_id=f.user_id AND i.feed_id=f.id AND i.unread=1),f.ordering,f.link,f.pinned,f.update_error_count,f.last_update_error,f.last_update,f.articles_per_update,f.update_enabled,f.update_mode,f.full_text_enabled,f.keyword_filters,f.credential_key_id,f.credential_nonce,f.credential_cipher FROM news_feeds f`

func (a *App) feeds(ctx context.Context, uid string) ([]Feed, error) {
	var out []Feed
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, feedSelect+` WHERE f.user_id=? ORDER BY f.pinned DESC,lower(f.title),f.id`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			f, err := a.scanFeed(uid, s)
			if err == nil {
				out = append(out, f)
			}
			return err
		}})
	})
	return out, err
}

func (a *App) feed(ctx context.Context, uid string, id int64) (Feed, error) {
	var out Feed
	found := false
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, feedSelect+` WHERE f.user_id=? AND f.id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error {
			var err error
			out, err = a.scanFeed(uid, s)
			found = err == nil
			return err
		}})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return out, err
}

func (a *App) insertFeed(ctx context.Context, uid string, f Feed) (Feed, error) {
	var id int64
	now := time.Now().Unix()
	err := a.withConn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err := sqlitex.Execute(c, `INSERT INTO news_feeds(user_id,folder_id,url,title,link,favicon_link,added,next_update,ordering,pinned,update_enabled,update_mode,full_text_enabled,keyword_filters) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, nullFolder(f.FolderID), f.URL, f.Title, f.Link, f.FaviconLink, now, now, f.Ordering, boolInt(f.Pinned), 1, f.UpdateMode, boolInt(f.FullTextEnabled), f.KeywordFilters}}); err != nil {
			return err
		}
		id = c.LastInsertRowID()
		if f.BasicAuthUser != "" || f.BasicAuthPassword != "" {
			if a.keyErr != nil {
				return a.keyErr
			}
			plain, err := json.Marshal([]string{f.BasicAuthUser, f.BasicAuthPassword})
			if err != nil {
				return err
			}
			keyID, nonce, cipherData, err := a.keys.Encrypt(string(plain), newsCredentialAAD(uid, id))
			if err != nil {
				return err
			}
			if err := sqlitex.Execute(c, `UPDATE news_feeds SET credential_key_id=?,credential_nonce=?,credential_cipher=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{keyID, nonce, cipherData, uid, id}}); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return Feed{}, err
	}
	return a.feed(ctx, uid, id)
}

func nullFolder(id int64) any {
	if id == 0 {
		return nil
	}
	return id
}

func (a *App) deleteFeed(ctx context.Context, uid string, id int64) error {
	defer a.markSearchDirty()
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		var err error
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM news_items WHERE user_id=? AND feed_id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}}); err != nil {
			return err
		}
		err = execOwned(c, `DELETE FROM news_feeds WHERE user_id=? AND id=?`, []any{uid, id})
		return err
	})
}

func (a *App) patchFeed(ctx context.Context, uid string, id int64, fields map[string]any) error {
	allowed := map[string]string{"title": "title", "folderId": "folder_id", "ordering": "ordering", "pinned": "pinned", "updateEnabled": "update_enabled", "updateMode": "update_mode", "fullTextEnabled": "full_text_enabled", "keywordFilters": "keyword_filters"}
	sets := []string{}
	args := []any{}
	for key, value := range fields {
		column, ok := allowed[key]
		if !ok {
			continue
		}
		if key == "folderId" {
			if n, ok := asInt64(value); ok {
				value = nullFolder(n)
			}
		}
		if b, ok := value.(bool); ok {
			value = boolInt(b)
		}
		sets = append(sets, column+"=?")
		args = append(args, value)
	}
	if len(sets) == 0 {
		return errInvalid
	}
	args = append(args, uid, id)
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return execOwned(c, `UPDATE news_feeds SET `+strings.Join(sets, ",")+` WHERE user_id=? AND id=?`, args)
	})
}

func asInt64(v any) (int64, bool) {
	switch n := v.(type) {
	case float64:
		return int64(n), true
	case int64:
		return n, true
	case int:
		return int64(n), true
	}
	return 0, false
}

func scanItem(s *sqlite.Stmt) Item {
	var author, enclosureMime, enclosureLink, thumb, description *string
	ptr := func(i int) *string {
		if s.ColumnType(i) == sqlite.TypeNull {
			return nil
		}
		v := s.ColumnText(i)
		return &v
	}
	author = ptr(5)
	enclosureMime = ptr(8)
	enclosureLink = ptr(9)
	thumb = ptr(10)
	description = ptr(11)
	return Item{ID: s.ColumnInt64(0), GUID: s.ColumnText(1), GUIDHash: s.ColumnText(2), URL: s.ColumnText(3), Title: s.ColumnText(4), Author: author, PubDate: s.ColumnInt64(6), Body: s.ColumnText(7), EnclosureMime: enclosureMime, EnclosureLink: enclosureLink, MediaThumbnail: thumb, MediaDescription: description, FeedID: s.ColumnInt64(12), Unread: s.ColumnInt64(13) != 0, Starred: s.ColumnInt64(14) != 0, RTL: s.ColumnInt64(15) != 0, LastModified: s.ColumnInt64(16), Fingerprint: s.ColumnText(17), ViewedAt: s.ColumnInt64(18)}
}

const itemSelect = `SELECT id,guid,guid_hash,url,title,author,pub_date,body,enclosure_mime,enclosure_link,media_thumbnail,media_description,feed_id,unread,starred,rtl,last_modified,fingerprint,viewed_at FROM news_items`

type itemQuery struct {
	Type         int
	ID           int64
	GetRead      bool
	BatchSize    int
	Offset       int64
	LastModified int64
	OldestFirst  bool
	Recent       bool
}

func (a *App) items(ctx context.Context, uid string, q itemQuery) ([]Item, error) {
	if q.BatchSize <= 0 {
		q.BatchSize = 100
	}
	if q.BatchSize > 200 {
		q.BatchSize = 200
	}
	where := []string{"user_id=?"}
	args := []any{uid}
	switch q.Type {
	case 0:
		where = append(where, "feed_id=?")
		args = append(args, q.ID)
	case 1:
		where = append(where, "feed_id IN (SELECT id FROM news_feeds WHERE user_id=? AND folder_id=?)")
		args = append(args, uid, q.ID)
	case 2:
		where = append(where, "starred=1")
	}
	if !q.GetRead {
		where = append(where, "unread=1")
	}
	if q.Offset > 0 {
		where = append(where, "id<=?")
		args = append(args, q.Offset)
	}
	if q.LastModified > 0 {
		where = append(where, "last_modified>=?")
		args = append(args, q.LastModified)
	}
	if q.Recent {
		where = append(where, "viewed_at>0")
	}
	order := " DESC"
	if q.OldestFirst {
		order = " ASC"
	}
	sql := itemSelect + ` WHERE ` + strings.Join(where, " AND ") + ` ORDER BY id` + order
	sql += ` LIMIT ?`
	args = append(args, q.BatchSize)
	var out []Item
	err := a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, sql, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error { out = append(out, scanItem(s)); return nil }})
	})
	return out, err
}

func (a *App) setItemState(ctx context.Context, uid string, ids []int64, column string, value bool) error {
	if column != "unread" && column != "starred" {
		return errInvalid
	}
	if len(ids) == 0 {
		return nil
	}
	marks := make([]string, len(ids))
	args := []any{boolInt(value), time.Now().Unix(), uid}
	for i, id := range ids {
		marks[i] = "?"
		args = append(args, id)
	}
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE news_items SET `+column+`=?,last_modified=? WHERE user_id=? AND id IN (`+strings.Join(marks, ",")+`)`, &sqlitex.ExecOptions{Args: args})
	})
}

func (a *App) markScopeRead(ctx context.Context, uid string, feedID, folderID, newest int64) error {
	where := []string{"user_id=?", "id<=?"}
	args := []any{time.Now().Unix(), uid, newest}
	if feedID > 0 {
		where = append(where, "feed_id=?")
		args = append(args, feedID)
	}
	if folderID > 0 {
		where = append(where, "feed_id IN (SELECT id FROM news_feeds WHERE user_id=? AND folder_id=?)")
		args = append(args, uid, folderID)
	}
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE news_items SET unread=0,last_modified=? WHERE `+strings.Join(where, " AND "), &sqlitex.ExecOptions{Args: args})
	})
}

func (a *App) viewItem(ctx context.Context, uid string, id int64) error {
	return a.withConn(ctx, func(c *sqlite.Conn) error {
		return execOwned(c, `UPDATE news_items SET viewed_at=?,unread=0,last_modified=? WHERE user_id=? AND id=?`, []any{time.Now().Unix(), time.Now().Unix(), uid, id})
	})
}

func (a *App) preferences(ctx context.Context, uid string) (Preferences, error) {
	p := defaultPreferences()
	err := a.withConn(ctx, func(c *sqlite.Conn) error { _, err := preferences.GetJSON(c, uid, "news", "settings", &p); return err })
	return p, err
}
func (a *App) savePreferences(ctx context.Context, uid string, p Preferences) error {
	valid := func(v string, values ...string) bool {
		for _, x := range values {
			if v == x {
				return true
			}
		}
		return false
	}
	if !valid(p.Thumbnails, "always", "ask", "never") || !valid(p.Enclosures, "always", "ask", "never") || !valid(p.Images, "always", "ask", "never") || !valid(p.Iframes, "always", "ask", "never") || !valid(p.DisplayMode, "default", "compact", "screenreader") || !valid(p.SplitMode, "vertical", "horizontal", "off") {
		return errInvalid
	}
	return a.withConn(ctx, func(c *sqlite.Conn) error { return preferences.SetJSON(c, uid, "news", "settings", p) })
}
