package notes

import (
	"bytes"
	"context"
	"embed"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"
	"os"
	"path"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
	"github.com/go-chi/chi/v5"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

//go:embed assets/*
var notesFS embed.FS
var notesScript = mustAsset("assets/app.js")

func mustAsset(name string) []byte {
	b, err := notesFS.ReadFile(name)
	if err != nil {
		panic(err)
	}
	return b
}

type Config struct {
	DB       *db.DB
	Files    FileStore
	Search   apps.SearchIndexWriter
	Registry *apps.Registry
}
type App struct {
	db       *db.DB
	files    FileStore
	scans    scanCoordinator
	search   apps.SearchIndexWriter
	registry *apps.Registry
}

func New(config Config) *App {
	return &App{db: config.DB, files: config.Files, scans: newScanCoordinator(), search: config.Search, registry: config.Registry}
}

func (a *App) recordActivity(ctx context.Context, userID, action, summary, objectType, objectID, objectName string) {
	if a.registry == nil || userID == "" {
		return
	}
	actorName := "User"
	if a.db != nil {
		if conn, release, err := a.db.Conn(ctx); err == nil {
			_ = sqlitex.Execute(conn, `SELECT display_name FROM users WHERE id=?`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				if strings.TrimSpace(stmt.ColumnText(0)) != "" {
					actorName = stmt.ColumnText(0)
				}
				return nil
			}})
			release()
		}
	}
	if err := a.registry.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: userID, ActorName: actorName, ActorKind: "user", Source: "notes",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/notes", Origin: "api", RecipientIDs: []string{userID},
	}); err != nil {
		log.Printf("record notes activity: %v", err)
	}
}
func (a *App) Name() string        { return "notes" }
func (a *App) DisplayName() string { return "Notes" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-note-sticky"></i>` }
func (a *App) Description() string { return "Markdown notes" }
func (a *App) AppBarOrder() int    { return 8 }

func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/notes/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handleAsset)},
		{Method: http.MethodGet, Pattern: "/notes", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/notes/new", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/notes/note/{id}", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: apps.MethodAny, Pattern: "/api/notes/notes", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNotesCollection)},
		{Method: apps.MethodAny, Pattern: "/api/notes/notes/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNote)},
		{Method: apps.MethodAny, Pattern: "/api/notes/categories", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCategories)},
		{Method: http.MethodPost, Pattern: "/api/notes/undo/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleUndo)},
		{Method: apps.MethodAny, Pattern: "/api/notes/versions/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleVersions)},
		{Method: apps.MethodAny, Pattern: "/api/notes/settings", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSettings)},
		{Method: apps.MethodAny, Pattern: "/api/notes/attachment/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleAttachment)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/notes/api/v1/notes", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNotesCollection)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/notes/api/v1/notes/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNote)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/notes/api/v1/settings", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSettings)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/notes/api/v1.4/attachment/{id}", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleAttachment)},
	}
}

func (a *App) handleAsset(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
	http.ServeContent(w, r, "app.js", time.Time{}, bytes.NewReader(notesScript))
}
func (a *App) handlePage(w http.ResponseWriter, r *http.Request) {
	u := server.GetUser(r)
	if u == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	if err := webui.RenderRequest(w, r, webui.PageData{Title: "Notes - Dar", CurrentApp: "notes", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(u.DisplayName), ContentTemplate: "notes.html", ContentData: struct{ UserID string }{UserID: u.ID}}); err != nil {
		http.Error(w, "Internal server error", 500)
	}
}

func apiHeaders(w http.ResponseWriter) { w.Header().Set("X-Notes-API-Versions", "1.4") }
func uidOr401(w http.ResponseWriter, r *http.Request) string {
	uid := ""
	if u := server.GetUser(r); u != nil {
		uid = u.ID
	}
	if uid == "" {
		http.Error(w, "unauthorized", 401)
	}
	return uid
}
func decodePatch(r *http.Request) (notePatch, error) {
	var p notePatch
	err := json.NewDecoder(io.LimitReader(r.Body, 16<<20)).Decode(&p)
	if errors.Is(err, io.EOF) {
		err = nil
	}
	return p, err
}
func noteMap(n Note, exclude map[string]bool) map[string]any {
	out := map[string]any{"id": n.ID, "etag": n.ETag, "readonly": false, "content": n.Content, "title": n.Title, "category": n.Category, "favorite": n.Favorite, "modified": n.Modified}
	for k := range exclude {
		delete(out, k)
	}
	return out
}

func noteExcerpt(content string) string {
	content = strings.NewReplacer("#", " ", "*", " ", "_", " ", "`", " ", ">", " ", "[", " ", "]", " ", "(", " ", ")", " ", "|", " ", "~", " ").Replace(content)
	text := strings.Join(strings.Fields(content), " ")
	runes := []rune(text)
	if len(runes) > 160 {
		return string(runes[:160]) + "…"
	}
	return text
}

func noteSummaryMap(n Note) map[string]any {
	return map[string]any{"id": n.ID, "etag": n.ETag, "readonly": false, "title": n.Title, "category": n.Category, "favorite": n.Favorite, "modified": n.Modified, "excerpt": noteExcerpt(n.Content)}
}
func excluded(r *http.Request) map[string]bool {
	m := map[string]bool{}
	for _, x := range strings.Split(r.URL.Query().Get("exclude"), ",") {
		if x = strings.TrimSpace(x); x != "" {
			m[x] = true
		}
	}
	return m
}
func aggregateETag(notes []Note) string {
	var b strings.Builder
	for _, n := range notes {
		b.WriteString(strconv.FormatInt(n.ID, 10))
		b.WriteString(n.ETag)
		b.WriteString(strconv.FormatBool(n.Tombstone))
	}
	return fingerprint([]byte(b.String()))[:32]
}

func (a *App) handleNotesCollection(w http.ResponseWriter, r *http.Request) {
	apiHeaders(w)
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	switch r.Method {
	case http.MethodGet:
		a.listNotes(w, r, uid)
	case http.MethodPost:
		p, err := decodePatch(r)
		if err != nil {
			http.Error(w, "invalid JSON", 400)
			return
		}
		n, err := a.create(r.Context(), uid, p)
		if err != nil {
			a.writeError(w, err)
			return
		}
		w.Header().Set("ETag", `"`+n.ETag+`"`)
		a.recordActivity(r.Context(), uid, "note_created", "created note "+n.Title, "note", strconv.FormatInt(n.ID, 10), n.Title)
		writeJSON(w, 200, n)
	default:
		http.Error(w, "method not allowed", 405)
	}
}
func (a *App) listNotes(w http.ResponseWriter, r *http.Request, uid string) {
	summary := r.URL.Query().Get("summary") == "1"
	notes, err := a.searchIndexedNotes(r.Context(), uid, true, r.URL.Query().Get("q"))
	if err != nil {
		a.writeError(w, err)
		return
	}
	category := r.URL.Query().Get("category")
	filtered := notes[:0]
	for _, n := range notes {
		if category == "" || n.Category == category {
			filtered = append(filtered, n)
		}
	}
	notes = filtered
	prune, _ := strconv.ParseInt(r.URL.Query().Get("pruneBefore"), 10, 64)
	size, _ := strconv.Atoi(r.URL.Query().Get("chunkSize"))
	offset, _ := strconv.Atoi(r.URL.Query().Get("chunkCursor"))
	if offset < 0 {
		offset = 0
	}
	var changed, pruned []Note
	for _, n := range notes {
		if n.Tombstone || (prune > 0 && n.Modified < prune) {
			pruned = append(pruned, n)
		} else {
			changed = append(changed, n)
		}
	}
	tag := aggregateETag(notes)
	if summary {
		tag = fingerprint([]byte("summary:" + aggregateETag(changed)))[:32]
	}
	w.Header().Set("ETag", `"`+tag+`"`)
	w.Header().Set("Last-Modified", time.Now().UTC().Format(http.TimeFormat))
	if strings.Trim(r.Header.Get("If-None-Match"), "\"") == tag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	full := changed
	finalChunk := true
	if size > 0 && offset < len(changed) {
		end := offset + size
		if end < len(changed) {
			full = changed[offset:end]
			finalChunk = false
			w.Header().Set("X-Notes-Chunk-Cursor", strconv.Itoa(end))
			w.Header().Set("X-Notes-Chunk-Pending", strconv.Itoa(len(changed)-end))
		} else {
			full = changed[offset:]
		}
	}
	ex := excluded(r)
	out := make([]map[string]any, 0, len(full)+len(pruned))
	for _, n := range full {
		if summary {
			out = append(out, noteSummaryMap(n))
		} else {
			out = append(out, noteMap(n, ex))
		}
	}
	if finalChunk && !summary {
		for _, n := range pruned {
			out = append(out, map[string]any{"id": n.ID})
		}
	}
	writeJSON(w, 200, out)
}

func routeID(r *http.Request) (int64, error) {
	raw := chi.URLParam(r, "id")
	if raw == "" {
		raw = r.URL.Query().Get("id")
	}
	return parseID(raw)
}
func (a *App) handleNote(w http.ResponseWriter, r *http.Request) {
	apiHeaders(w)
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	id, err := routeID(r)
	if err != nil {
		http.Error(w, "invalid id", 400)
		return
	}
	switch r.Method {
	case http.MethodGet:
		n, err := a.note(r.Context(), uid, id)
		if err != nil {
			a.writeError(w, err)
			return
		}
		w.Header().Set("ETag", `"`+n.ETag+`"`)
		if strings.Trim(r.Header.Get("If-None-Match"), "\"") == n.ETag {
			w.WriteHeader(304)
			return
		}
		writeJSON(w, 200, n)
	case http.MethodPut:
		p, err := decodePatch(r)
		if err != nil {
			http.Error(w, "invalid JSON", 400)
			return
		}
		n, err := a.update(r.Context(), uid, id, p, r.Header.Get("If-Match"), r.URL.Query().Get("force") == "1")
		if errors.Is(err, errConflict) {
			writeJSON(w, 412, n)
			return
		}
		if err != nil {
			a.writeError(w, err)
			return
		}
		w.Header().Set("ETag", `"`+n.ETag+`"`)
		a.recordActivity(r.Context(), uid, "note_updated", "updated note "+n.Title, "note", strconv.FormatInt(n.ID, 10), n.Title)
		writeJSON(w, 200, n)
	case http.MethodDelete:
		if err := a.delete(r.Context(), uid, id); err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "note_deleted", "deleted note "+strconv.FormatInt(id, 10), "note", strconv.FormatInt(id, 10), strconv.FormatInt(id, 10))
		writeJSON(w, 200, map[string]string{"status": "deleted"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (a *App) handleSettings(w http.ResponseWriter, r *http.Request) {
	apiHeaders(w)
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	s, err := a.settings(r.Context(), uid)
	if err != nil {
		a.writeError(w, err)
		return
	}
	if r.Method == http.MethodPut {
		var in Settings
		if err = json.NewDecoder(r.Body).Decode(&in); err != nil {
			http.Error(w, "invalid JSON", 400)
			return
		}
		if in.NotesPath != "" {
			s.NotesPath = in.NotesPath
		}
		if in.FileSuffix != "" {
			s.FileSuffix = in.FileSuffix
		}
		if in.EditorMode != "" {
			s.EditorMode = in.EditorMode
		}
		s = sanitizeSettings(s)
		if err = a.saveSettings(r.Context(), uid, s); err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "settings_updated", "updated Notes settings", "preferences", uid, "Notes settings")
	} else if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", 405)
		return
	}
	writeJSON(w, 200, s)
}

func categories(ctx context.Context, a *App, uid string) ([]string, error) {
	s, err := a.settings(ctx, uid)
	if err != nil {
		return nil, err
	}
	if err = a.files.MkdirAll(ctx, uid, s.NotesPath); err != nil {
		return nil, err
	}
	set := map[string]bool{}
	err = a.files.Walk(ctx, uid, s.NotesPath, func(vpath string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if info.IsDir() && vpath != s.NotesPath {
			set[strings.TrimPrefix(strings.TrimPrefix(vpath, s.NotesPath), "/")] = true
		}
		return nil
	})
	out := make([]string, 0, len(set))
	for x := range set {
		out = append(out, x)
	}
	sort.Strings(out)
	return out, err
}

func validCategoryName(name string) bool {
	if name == "" || strings.HasPrefix(name, "/") || strings.Contains(name, `\`) || path.Clean(name) != name {
		return false
	}
	for _, part := range strings.Split(name, "/") {
		if part == "" || safePart(part) != part {
			return false
		}
	}
	return true
}

func (a *App) handleCategories(w http.ResponseWriter, r *http.Request) {
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	s, err := a.settings(r.Context(), uid)
	if err != nil {
		a.writeError(w, err)
		return
	}
	switch r.Method {
	case http.MethodGet:
		out, err := categories(r.Context(), a, uid)
		if err != nil {
			a.writeError(w, err)
			return
		}
		writeJSON(w, 200, map[string]any{"categories": out})
	case http.MethodPost:
		var in struct {
			Name string `json:"name"`
		}
		if json.NewDecoder(r.Body).Decode(&in) != nil || !validCategoryName(in.Name) {
			http.Error(w, "invalid category", 400)
			return
		}
		if err = a.files.MkdirAll(r.Context(), uid, path.Join(s.NotesPath, in.Name)); err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "category_created", "created note category "+in.Name, "category", in.Name, in.Name)
		writeJSON(w, 200, map[string]string{"name": strings.Trim(in.Name, "/")})
	case http.MethodPut:
		var in struct {
			From string `json:"from"`
			To   string `json:"to"`
		}
		if json.NewDecoder(r.Body).Decode(&in) != nil || !validCategoryName(in.From) || !validCategoryName(in.To) {
			http.Error(w, "invalid category", 400)
			return
		}
		if err = a.renameCategory(r.Context(), uid, in.From, in.To); err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "category_renamed", "renamed note category to "+in.To, "category", in.To, in.To)
		writeJSON(w, 200, map[string]string{"name": in.To})
	case http.MethodDelete:
		name := r.URL.Query().Get("name")
		if !validCategoryName(name) {
			http.Error(w, "invalid category", 400)
			return
		}
		force := r.URL.Query().Get("force") == "1"
		if err = a.deleteCategory(r.Context(), uid, name, force); errors.Is(err, errCategoryNotEmpty) {
			http.Error(w, "category is not empty", 409)
			return
		} else if err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "category_deleted", "deleted note category "+name, "category", name, name)
		writeJSON(w, 200, map[string]string{"status": "deleted"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (a *App) handleUndo(w http.ResponseWriter, r *http.Request) {
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	id, err := routeID(r)
	if err != nil {
		http.Error(w, "invalid id", 400)
		return
	}
	n, err := a.undo(r.Context(), uid, id)
	if err != nil {
		a.writeError(w, err)
		return
	}
	a.recordActivity(r.Context(), uid, "note_restored", "restored note "+n.Title, "note", strconv.FormatInt(n.ID, 10), n.Title)
	writeJSON(w, 200, n)
}
func (a *App) handleVersions(w http.ResponseWriter, r *http.Request) {
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	id, err := routeID(r)
	if err != nil {
		http.Error(w, "invalid id", 400)
		return
	}
	n, err := a.note(r.Context(), uid, id)
	if err != nil {
		a.writeError(w, err)
		return
	}
	if r.Method == http.MethodGet {
		ids, err := a.files.VersionIDs(r.Context(), uid, n.Path)
		if err != nil {
			a.writeError(w, err)
			return
		}
		writeJSON(w, 200, map[string]any{"versions": ids})
		return
	}
	if r.Method == http.MethodPost {
		version := r.URL.Query().Get("version")
		if version == "" {
			http.Error(w, "version required", 400)
			return
		}
		data, err := a.files.ReadSnapshot(r.Context(), uid, n.Path, version)
		if err != nil {
			a.writeError(w, err)
			return
		}
		content := string(data)
		restored, err := a.update(r.Context(), uid, id, notePatch{Content: &content}, n.ETag, true)
		if err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "version_restored", "restored a version of "+restored.Title, "note", strconv.FormatInt(restored.ID, 10), restored.Title)
		writeJSON(w, 200, restored)
		return
	}
	http.Error(w, "method not allowed", 405)
}

func (a *App) handleAttachment(w http.ResponseWriter, r *http.Request) {
	apiHeaders(w)
	uid := uidOr401(w, r)
	if uid == "" {
		return
	}
	id, err := routeID(r)
	if err != nil {
		http.Error(w, "invalid id", 400)
		return
	}
	n, err := a.note(r.Context(), uid, id)
	if err != nil {
		a.writeError(w, err)
		return
	}
	dir := path.Dir(n.Path)
	if r.Method == http.MethodGet {
		rawPath := r.URL.Query().Get("path")
		name := path.Base(rawPath)
		if rawPath != name || name == "." || name == "/" || name == "" {
			http.Error(w, "path required", 400)
			return
		}
		data, err := a.files.ReadFile(r.Context(), uid, path.Join(dir, name))
		if err != nil {
			a.writeError(w, err)
			return
		}
		w.Header().Set("Content-Type", http.DetectContentType(data))
		w.Write(data)
		return
	}
	if r.Method == http.MethodPost {
		r.Body = http.MaxBytesReader(w, r.Body, 10<<20)
		if err = r.ParseMultipartForm(10 << 20); err != nil {
			http.Error(w, "image too large", 413)
			return
		}
		file, header, err := r.FormFile("file")
		if err != nil {
			http.Error(w, "file required", 400)
			return
		}
		defer file.Close()
		data, err := io.ReadAll(io.LimitReader(file, 10<<20+1))
		if err != nil || len(data) > 10<<20 {
			http.Error(w, "image too large", 413)
			return
		}
		_, ext, ok := validImageType(data, header.Filename)
		if !ok {
			http.Error(w, "unsupported image", 415)
			return
		}
		name := randomName(ext)
		if err = a.files.WriteFile(r.Context(), uid, path.Join(dir, name), data); err != nil {
			a.writeError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "attachment_added", "added an attachment to "+n.Title, "note", strconv.FormatInt(n.ID, 10), n.Title)
		writeJSON(w, 200, map[string]string{"filename": name})
		return
	}
	http.Error(w, "method not allowed", 405)
}

func (a *App) writeError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, os.ErrNotExist):
		http.Error(w, "not found", 404)
	case strings.Contains(err.Error(), "locked"):
		http.Error(w, "locked", 423)
	default:
		http.Error(w, "internal server error", 500)
	}
}
