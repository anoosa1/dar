package files

import (
	"context"
	"fmt"
	"io/fs"
	"mime"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"github.com/anoosa/dar/internal/apps"
	searchindex "github.com/anoosa/dar/internal/search"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func (app *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	fileTags := app.loadFileTags(uid)
	tagDefs := app.loadTags(uid)
	if err := app.indexSearchRoot(ctx, sink, uid, uid, app.userHome(uid), "", fileTags, tagDefs); err != nil {
		return err
	}
	if app.db == nil {
		return nil
	}
	type sharedRoot struct {
		id, owner, path string
	}
	var shares []sharedRoot
	conn, release, err := app.db.Conn(ctx)
	if err != nil {
		return err
	}
	err = sqlitex.Execute(conn, `SELECT id,from_user,path FROM files_user_shares WHERE to_user=? ORDER BY id`,
		&sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(stmt *sqlite.Stmt) error {
			shares = append(shares, sharedRoot{id: stmt.ColumnText(0), owner: stmt.ColumnText(1), path: stmt.ColumnText(2)})
			return nil
		}})
	release()
	if err != nil {
		return err
	}
	for _, share := range shares {
		prefix := "share:" + share.id + ":"
		if err := app.indexSearchRoot(ctx, sink, uid, share.owner, app.realPath(share.owner, share.path), prefix, nil, nil); err != nil && !os.IsNotExist(err) {
			return err
		}
	}
	return nil
}

func (app *App) indexSearchRoot(ctx context.Context, sink apps.SearchDocumentSink, searchUser, owner, root, objectPrefix string, fileTags map[string][]string, tagDefs map[string]Tag) error {
	home := app.userHome(owner)
	start, err := filepath.Rel(home, root)
	if err != nil || start == ".." || strings.HasPrefix(start, ".."+string(filepath.Separator)) {
		return fmt.Errorf("search root is outside the owner Files root")
	}
	if start == "" {
		start = "."
	}
	userRoot, err := os.OpenRoot(home)
	if err != nil {
		return err
	}
	defer userRoot.Close()
	return fs.WalkDir(userRoot.FS(), start, func(name string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			if os.IsNotExist(walkErr) {
				return nil
			}
			return walkErr
		}
		if err := ctx.Err(); err != nil {
			return err
		}
		if name == start && objectPrefix == "" {
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		vpath := "/" + filepath.ToSlash(name)
		if objectPrefix != "" {
			shareID := strings.TrimSuffix(strings.TrimPrefix(objectPrefix, "share:"), ":")
			sharedRel, relErr := filepath.Rel(start, name)
			if relErr != nil {
				return relErr
			}
			vpath = path.Join("/Shared", shareID, filepath.ToSlash(sharedRel))
		}
		if strings.Contains(vpath, "/.versions/") || strings.Contains(vpath, "/.trash/") {
			if info.IsDir() {
				return fs.SkipDir
			}
			return nil
		}
		objectID := objectPrefix + vpath
		title := info.Name()
		keywords := vpath + " " + mime.TypeByExtension(strings.ToLower(filepath.Ext(title)))
		for _, tagID := range fileTags[vpath] {
			keywords += " " + tagDefs[tagID].Name
		}
		body := ""
		if !info.IsDir() {
			if data, readErr := userRoot.ReadFile(name); readErr == nil && utf8.Valid(data) && !strings.ContainsRune(string(data), '\x00') {
				body = string(data)
			}
		}
		chunks := searchindex.ChunkText(body)
		for i, chunk := range chunks {
			doc := apps.SearchDocument{
				ObjectID: objectID, ChunkID: fmt.Sprintf("%d", i), Title: title,
				Subtitle: vpath, Keywords: keywords, Body: chunk,
				URL: "/files" + escapeFilePath(vpath), Anchor: "files-app",
				ModifiedAt: info.ModTime().Unix(),
			}
			if err := sink.AddSearchDocument(ctx, doc); err != nil {
				return err
			}
		}
		return nil
	})
}

func escapeFilePath(value string) string {
	parts := strings.Split(strings.TrimPrefix(value, "/"), "/")
	for i := range parts {
		parts[i] = url.PathEscape(parts[i])
	}
	if len(parts) == 1 && parts[0] == "" {
		return ""
	}
	return "/" + strings.Join(parts, "/")
}

func (app *App) markSearchDirty() {
	if app.search != nil {
		app.search.MarkSearchDirty("files")
	}
}
