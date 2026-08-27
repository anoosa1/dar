// Files app backend: disk-based storage with trash, favorites, text files, and copy.
package files

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"image"
	"image/jpeg"
	_ "image/png"
	"io"
	"io/fs"
	"log"
	"mime"
	"mime/multipart"
	"net"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var filesTheme = theme.MochaPink()

type FileEntry struct {
	Name      string   `json:"name"`
	Path      string   `json:"path"`
	Size      int64    `json:"size"`
	MimeType  string   `json:"mime_type"`
	IsDir     bool     `json:"is_dir"`
	UpdatedAt int64    `json:"updated_at"`
	Favorited bool     `json:"favorited"`
	Tags      []string `json:"tags"`
	TrashPath string   `json:"trash_path,omitempty"`
}

type Tag struct {
	ID    string `json:"id"`
	Name  string `json:"name"`
	Color string `json:"color"`
}
type UserShare struct {
	ID         string `json:"id"`
	Path       string `json:"path"`
	FromUser   string `json:"from_user"`
	ToUser     string `json:"to_user"`
	Permission string `json:"permission"`
	Created    int64  `json:"created"`
}

type FileResponse struct {
	Files []FileEntry `json:"files"`
	Path  string      `json:"path"`
}

const editorFileLimit = 1 << 20

type editorDocument struct {
	Path     string `json:"path"`
	Name     string `json:"name"`
	Content  string `json:"content"`
	ETag     string `json:"etag"`
	Markdown bool   `json:"markdown"`
	MimeType string `json:"mime_type"`
	Size     int64  `json:"size"`
}

// PickerRequest is the small, stable contract consumed by DarFilePicker.
// Paths are always user-rooted virtual paths, never host filesystem paths.
type PickerRequest struct {
	Path         string   `json:"path"`
	Query        string   `json:"query"`
	Selection    string   `json:"selection"` // files|folders|both
	Extensions   []string `json:"extensions,omitempty"`
	MimePrefixes []string `json:"mime_prefixes,omitempty"`
}

func (app *App) userHome(uid string) string {
	d := filepath.Join(app.dataRoot, uid, "home")
	os.MkdirAll(d, 0700)
	return d
}

func (app *App) cleanVPath(vpath string) string {
	vpath = strings.TrimSpace(vpath)
	if vpath == "" {
		return "/"
	}
	if !strings.HasPrefix(vpath, "/") {
		vpath = "/" + vpath
	}
	vpath = path.Clean(vpath)
	if vpath == "." {
		return "/"
	}
	return vpath
}

func (app *App) trashPathFromRequest(r *http.Request) string {
	vpath := r.URL.Query().Get("trash_path")
	if strings.TrimSpace(vpath) == "" {
		return "/.local/share/Trash"
	}
	return app.cleanVPath(vpath)
}

func (app *App) trashStoreDir(uid, trashPath string) string {
	trashPath = app.cleanVPath(trashPath)
	d := app.realPath(uid, trashPath)
	os.MkdirAll(d, 0700)
	return d
}

func (app *App) realPath(uid, vpath string) string {
	vpath = app.cleanVPath(vpath)
	return app.safeJoin(app.userHome(uid), vpath)
}

func (app *App) safeJoin(root, vpath string) string {
	rel := filepath.Clean(strings.TrimPrefix(vpath, "/"))
	if rel == "." {
		return root
	}
	if rel == ".." || strings.HasPrefix(rel, "../") {
		return root
	}
	return filepath.Join(root, rel)
}

func (app *App) userID(r *http.Request) string {
	if user := server.GetUser(r); user != nil {
		return user.ID
	}
	return ""
}

func (app *App) loadFavorites(uid string) map[string]bool {
	m := map[string]bool{}
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT path FROM files_favorites WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { m[s.ColumnText(0)] = true; return nil }})
	})
	return m
}
func (app *App) saveFavorites(uid string, m map[string]bool) {
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM files_favorites WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}}); err != nil {
			return err
		}
		for p, favorite := range m {
			if favorite {
				if err = sqlitex.Execute(c, `INSERT INTO files_favorites(user_id,path) VALUES(?,?)`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(p)}}); err != nil {
					return err
				}
			}
		}
		return nil
	})
}

func (app *App) handleList(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := app.cleanVPath(r.URL.Query().Get("path"))
	trashPath := app.trashPathFromRequest(r)
	filter := r.URL.Query().Get("filter")
	favs := app.loadFavorites(uid)
	fileTags := app.loadFileTags(uid)

	// Trash view
	if vpath == trashPath {
		userRoot, err := os.OpenRoot(app.userHome(uid))
		if err != nil {
			http.Error(w, "trash unavailable", http.StatusInternalServerError)
			return
		}
		defer userRoot.Close()
		trashRoot := strings.TrimPrefix(trashPath, "/")
		if err := userRoot.MkdirAll(trashRoot, 0700); err != nil {
			http.Error(w, "trash unavailable", http.StatusInternalServerError)
			return
		}
		var files []FileEntry
		records, _ := app.trashRecords(r.Context(), uid)
		for _, record := range records {
			info, err := userRoot.Stat(filepath.Join(trashRoot, record.ID))
			if err != nil {
				continue
			}
			files = append(files, FileEntry{
				Name: filepath.Base(record.OriginalPath), Path: record.OriginalPath, Size: info.Size(),
				IsDir: info.IsDir(), UpdatedAt: record.DeletedAt, TrashPath: record.ID,
			})
		}
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		app.writeJSON(w, FileResponse{Files: files, Path: trashPath})
		return
	}

	// Favorites filter
	if filter == "favorites" {
		root := app.userHome(uid)
		var files []FileEntry
		filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
			if err != nil || p == root {
				return nil
			}
			rel, _ := filepath.Rel(root, p)
			vp := "/" + filepath.ToSlash(rel)
			if info.IsDir() && favs[vp] {
				files = append(files, app.makeEntry(info, vp, favs, fileTags))
				return filepath.SkipDir
			}
			if favs[vp] {
				files = append(files, app.makeEntry(info, vp, favs, fileTags))
			}
			return nil
		})
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		app.writeJSON(w, FileResponse{Files: files, Path: "/"})
		return
	}

	// Tag filter
	if filter == "tag" {
		tagID := r.URL.Query().Get("tag")
		var files []FileEntry
		app.addTaggedFiles(uid, app.userHome(uid), "", fileTags, tagID, favs, &files)
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		app.writeJSON(w, FileResponse{Files: files, Path: "/"})
		return
	}

	if vpath == "/Shared" {
		shares := app.loadIncomingShares(uid)
		var files []FileEntry
		for _, s := range shares {
			info, err := app.Stat(r.Context(), s.FromUser, s.Path)
			if err != nil {
				continue
			}
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: "/Shared/" + s.ID, Size: info.Size(), IsDir: info.IsDir(), UpdatedAt: info.ModTime().Unix()})
		}
		if files == nil {
			files = []FileEntry{}
		}
		app.writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
	if err != nil {
		http.Error(w, "folder not found", http.StatusNotFound)
		return
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		http.Error(w, "could not read folder", http.StatusInternalServerError)
		return
	}
	defer root.Close()
	rootName := strings.TrimPrefix(resolved.OwnerPath, "/")
	if rootName == "" {
		rootName = "."
	}
	entries, err := fs.ReadDir(root.FS(), rootName)
	if err != nil {
		http.Error(w, "folder not found", http.StatusNotFound)
		return
	}
	var files []FileEntry
	for _, e := range entries {
		info, infoErr := e.Info()
		if e.Type()&os.ModeSymlink != 0 {
			// Follow user links for normal browsing, but keep the lookup
			// confined by os.Root. Regular entries avoid an extra syscall.
			info, infoErr = root.Stat(path.Join(strings.TrimPrefix(resolved.OwnerPath, "/"), e.Name()))
		}
		if infoErr != nil {
			continue
		}
		childPath := vpath + "/" + e.Name()
		if vpath == "/" || vpath == "" {
			childPath = "/" + e.Name()
		}
		files = append(files, app.makeEntry(info, childPath, favs, fileTags))
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir {
			return files[i].IsDir
		}
		return strings.ToLower(files[i].Name) < strings.ToLower(files[j].Name)
	})
	if (vpath == "/" || vpath == "") && len(app.loadIncomingShares(uid)) > 0 {
		hasShared := false
		for _, f := range files {
			if f.Name == "Shared" {
				hasShared = true
				break
			}
		}
		if !hasShared {
			files = append([]FileEntry{{Name: "Shared", Path: "/Shared", IsDir: true, UpdatedAt: time.Now().Unix()}}, files...)
		}
	}
	if files == nil {
		files = []FileEntry{}
	}
	app.writeJSON(w, FileResponse{Files: files, Path: vpath})
}

// handlePicker lists one folder for the shared cross-app picker. Unlike the
// Files UI it deliberately excludes trash, virtual filters, and mutations.
func (app *App) handlePicker(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	req := PickerRequest{
		Path: r.URL.Query().Get("path"), Query: strings.TrimSpace(strings.ToLower(r.URL.Query().Get("q"))),
		Selection: r.URL.Query().Get("selection"), MimePrefixes: r.URL.Query()["mime_prefix"],
	}
	if req.Selection == "" {
		req.Selection = "both"
	}
	if req.Selection != "files" && req.Selection != "folders" && req.Selection != "both" {
		http.Error(w, "invalid selection", http.StatusBadRequest)
		return
	}
	if !validMIMEPrefixes(req.MimePrefixes) {
		http.Error(w, "invalid MIME filter", http.StatusBadRequest)
		return
	}
	vpath := app.cleanVPath(req.Path)
	// The picker only traverses the authenticated user's normal Files root.
	if vpath == "/.local/share/Trash" || strings.HasPrefix(vpath, "/.local/share/Trash/") {
		http.Error(w, "trash cannot be selected", http.StatusBadRequest)
		return
	}
	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
	if err != nil {
		http.Error(w, "folder not found", http.StatusNotFound)
		return
	}
	userRoot, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		http.Error(w, "folder not found", http.StatusNotFound)
		return
	}
	defer userRoot.Close()
	ownerFolder := strings.TrimPrefix(resolved.OwnerPath, "/")
	if ownerFolder == "" {
		ownerFolder = "."
	}
	entries, err := fs.ReadDir(userRoot.FS(), ownerFolder)
	if err != nil {
		http.Error(w, "folder not found", http.StatusNotFound)
		return
	}
	favs, tags := app.loadFavorites(uid), app.loadFileTags(uid)
	files := make([]FileEntry, 0, len(entries))
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".") {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		if req.Query != "" && !strings.Contains(strings.ToLower(entry.Name()), req.Query) {
			continue
		}
		if req.Selection == "files" && info.IsDir() { /* retain folders for navigation */
		} else if req.Selection == "folders" && !info.IsDir() {
			continue
		}
		child := path.Join(vpath, entry.Name())
		if !strings.HasPrefix(child, "/") {
			child = "/" + child
		}
		item := app.makeEntry(info, child, favs, tags)
		if !info.IsDir() && len(req.MimePrefixes) > 0 {
			item.MimeType = detectedMIMEFromRoot(userRoot, path.Join(ownerFolder, entry.Name()), info.Name())
			if !matchesMIMEPrefix(item.MimeType, req.MimePrefixes) {
				continue
			}
		}
		files = append(files, item)
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir {
			return files[i].IsDir
		}
		return strings.ToLower(files[i].Name) < strings.ToLower(files[j].Name)
	})
	app.writeJSON(w, FileResponse{Files: files, Path: vpath})
}

func (app *App) handlePickerValidate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Paths        []string `json:"paths"`
		Selection    string   `json:"selection"`
		Multiple     bool     `json:"multiple"`
		MimePrefixes []string `json:"mime_prefixes"`
	}
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "invalid json", http.StatusBadRequest)
		return
	}
	if body.Selection == "" {
		body.Selection = "both"
	}
	if body.Selection != "files" && body.Selection != "folders" && body.Selection != "both" {
		http.Error(w, "invalid selection", http.StatusBadRequest)
		return
	}
	if !validMIMEPrefixes(body.MimePrefixes) {
		http.Error(w, "invalid MIME filter", http.StatusBadRequest)
		return
	}
	if len(body.Paths) == 0 || (!body.Multiple && len(body.Paths) != 1) {
		http.Error(w, "invalid number of selections", http.StatusBadRequest)
		return
	}
	uid := app.userID(r)
	files := make([]FileEntry, 0, len(body.Paths))
	favs, tags := app.loadFavorites(uid), app.loadFileTags(uid)
	seen := map[string]bool{}
	for _, raw := range body.Paths {
		vp := app.cleanVPath(raw)
		if seen[vp] || vp != raw {
			http.Error(w, "invalid path", http.StatusBadRequest)
			return
		}
		seen[vp] = true
		info, err := app.Stat(r.Context(), uid, vp)
		if err != nil {
			http.Error(w, "selection not found", http.StatusNotFound)
			return
		}
		if (body.Selection == "files" && info.IsDir()) || (body.Selection == "folders" && !info.IsDir()) {
			http.Error(w, "selection type not allowed", http.StatusBadRequest)
			return
		}
		item := app.makeEntry(info, vp, favs, tags)
		if !info.IsDir() && len(body.MimePrefixes) > 0 {
			item.MimeType = app.detectedUserMIME(r.Context(), uid, vp, info.Name())
			if !matchesMIMEPrefix(item.MimeType, body.MimePrefixes) {
				http.Error(w, "selection MIME type not allowed", http.StatusBadRequest)
				return
			}
		}
		files = append(files, item)
	}
	app.writeJSON(w, map[string]any{"files": files})
}

func validMIMEPrefixes(prefixes []string) bool {
	if len(prefixes) > 8 {
		return false
	}
	for _, prefix := range prefixes {
		if prefix == "" || prefix != strings.ToLower(prefix) || !strings.Contains(prefix, "/") {
			return false
		}
		for _, r := range prefix {
			if !((r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') || strings.ContainsRune("!#$&^_.+-/", r)) {
				return false
			}
		}
	}
	return true
}

func detectedMIMEFromRoot(root *os.Root, rootPath, name string) string {
	if mediaType := mime.TypeByExtension(strings.ToLower(filepath.Ext(name))); mediaType != "" {
		return mediaType
	}
	file, err := root.Open(rootPath)
	if err != nil {
		return ""
	}
	defer file.Close()
	buffer := make([]byte, 512)
	n, _ := file.Read(buffer)
	if n == 0 {
		return "text/plain; charset=utf-8"
	}
	return http.DetectContentType(buffer[:n])
}

func (app *App) detectedUserMIME(ctx context.Context, uid, vpath, name string) string {
	resolved, err := app.resolveUserPath(ctx, uid, vpath, false)
	if err != nil {
		return ""
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		return ""
	}
	defer root.Close()
	rootPath := strings.TrimPrefix(resolved.OwnerPath, "/")
	if rootPath == "" {
		rootPath = "."
	}
	return detectedMIMEFromRoot(root, rootPath, name)
}

func matchesMIMEPrefix(mediaType string, prefixes []string) bool {
	mediaType = strings.ToLower(mediaType)
	for _, prefix := range prefixes {
		if strings.HasPrefix(mediaType, prefix) {
			return true
		}
	}
	return false
}

func (app *App) makeEntry(info os.FileInfo, path string, favs map[string]bool, ftags map[string][]string) FileEntry {
	mtype := ""
	if !info.IsDir() {
		mtype = mime.TypeByExtension(filepath.Ext(info.Name()))
	}
	return FileEntry{
		Name: info.Name(), Path: path, Size: info.Size(), MimeType: mtype,
		IsDir: info.IsDir(), UpdatedAt: info.ModTime().Unix(), Favorited: favs[path],
		Tags: ftags[path],
	}
}

func (app *App) handleUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	vpath = app.cleanVPath(vpath)
	mr, err := r.MultipartReader()
	if err != nil {
		http.Error(w, "invalid multipart upload", http.StatusBadRequest)
		return
	}
	for {
		part, err := mr.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			http.Error(w, "invalid multipart upload", http.StatusBadRequest)
			return
		}
		name, nameErr := uploadedFilename(part)
		if nameErr != nil {
			_ = part.Close()
			http.Error(w, "invalid filename", http.StatusBadRequest)
			return
		}
		if name == "" {
			_ = part.Close()
			continue
		}
		if err := app.saveUploadedStream(r.Context(), uid, vpath, name, part); err != nil {
			_ = part.Close()
			app.writeMutationError(w, err)
			return
		}
		_ = part.Close()
	}
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func uploadedFilename(part *multipart.Part) (string, error) {
	_, params, err := mime.ParseMediaType(part.Header.Get("Content-Disposition"))
	if err != nil {
		return "", err
	}
	name := strings.TrimSpace(params["filename"])
	if name == "" {
		return "", nil
	}
	if name != filepath.Base(name) || strings.ContainsAny(name, "/\\\x00") || name == "." || name == ".." {
		return "", fmt.Errorf("invalid filename")
	}
	return name, nil
}

func (app *App) saveUploadedStream(ctx context.Context, uid, vpath, name string, source io.Reader) error {
	name = strings.TrimSpace(name)
	if name == "" || name != filepath.Base(name) || strings.ContainsAny(name, "/\\\x00") {
		return fmt.Errorf("invalid filename")
	}
	fullPath := vpath + "/" + name
	fullPath = app.cleanVPath(fullPath)
	if _, err := app.Stat(ctx, uid, fullPath); err == nil {
		app.saveVersion(uid, fullPath)
	}
	if err := app.WriteStream(ctx, uid, fullPath, source); err != nil {
		return err
	}
	app.logActivity(uid, fullPath, name, "uploaded")
	return nil
}

func (app *App) handleDownload(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer root.Close()
	name := strings.TrimPrefix(resolved.OwnerPath, "/")
	file, err := root.Open(name)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil || info.IsDir() {
		http.NotFound(w, r)
		return
	}
	http.ServeContent(w, r, filepath.Base(resolved.OwnerPath), info.ModTime(), file)
}

func editorETag(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func editorPath(app *App, raw string) (string, bool) {
	if raw == "" || !strings.HasPrefix(raw, "/") || strings.ContainsRune(raw, '\x00') {
		return "", false
	}
	cleaned := app.cleanVPath(raw)
	return cleaned, cleaned == raw && cleaned != "/"
}

func editorName(name string) bool {
	return name != "" && name == filepath.Base(name) && name != "." && name != ".." &&
		!strings.ContainsAny(name, "/\\\x00")
}

func editorMime(name string, data []byte) string {
	if mediaType := mime.TypeByExtension(strings.ToLower(filepath.Ext(name))); mediaType != "" {
		return mediaType
	}
	if len(data) == 0 {
		return "text/plain; charset=utf-8"
	}
	return http.DetectContentType(data[:min(len(data), 512)])
}

func editorMarkdown(name string) bool {
	switch strings.ToLower(filepath.Ext(name)) {
	case ".md", ".markdown":
		return true
	default:
		return false
	}
}

func makeEditorDocument(vpath string, data []byte) editorDocument {
	name := filepath.Base(vpath)
	return editorDocument{
		Path: vpath, Name: name, Content: string(data), ETag: editorETag(data),
		Markdown: editorMarkdown(name), MimeType: editorMime(name, data), Size: int64(len(data)),
	}
}

func readEditorDocument(app *App, uid, vpath string) (editorDocument, int, error) {
	info, err := app.Stat(context.Background(), uid, vpath)
	if err != nil {
		if os.IsNotExist(err) {
			return editorDocument{}, http.StatusNotFound, err
		}
		return editorDocument{}, http.StatusInternalServerError, err
	}
	if info.IsDir() {
		return editorDocument{}, http.StatusUnsupportedMediaType, fmt.Errorf("folders cannot be edited")
	}
	if info.Size() > editorFileLimit {
		return editorDocument{}, http.StatusRequestEntityTooLarge, fmt.Errorf("file is too large to edit")
	}
	data, err := app.ReadFile(context.Background(), uid, vpath)
	if err != nil {
		return editorDocument{}, http.StatusInternalServerError, err
	}
	if len(data) > editorFileLimit {
		return editorDocument{}, http.StatusRequestEntityTooLarge, fmt.Errorf("file is too large to edit")
	}
	if !utf8.Valid(data) || bytes.ContainsRune(data, '\x00') {
		return editorDocument{}, http.StatusUnsupportedMediaType, fmt.Errorf("file is not UTF-8 text")
	}
	return makeEditorDocument(vpath, data), http.StatusOK, nil
}

func (app *App) writeEditorError(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	app.writeJSON(w, map[string]string{"error": message})
}

func (app *App) writeEditorDocument(w http.ResponseWriter, status int, document editorDocument) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("ETag", `"`+document.ETag+`"`)
	w.WriteHeader(status)
	app.writeJSON(w, document)
}

func normalizedIfMatch(value string) (string, bool) {
	value = strings.TrimSpace(value)
	if value == "" || strings.Contains(value, ",") || strings.HasPrefix(value, "W/") {
		return "", false
	}
	if strings.HasPrefix(value, `"`) && strings.HasSuffix(value, `"`) && len(value) >= 2 {
		value = value[1 : len(value)-1]
	}
	return value, value != ""
}

func (app *App) handleEditor(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath, ok := editorPath(app, r.URL.Query().Get("path"))
	if !ok {
		app.writeEditorError(w, http.StatusBadRequest, "invalid path")
		return
	}
	switch r.Method {
	case http.MethodGet:
		document, status, err := readEditorDocument(app, uid, vpath)
		if err != nil {
			app.writeEditorError(w, status, err.Error())
			return
		}
		app.writeEditorDocument(w, http.StatusOK, document)
	case http.MethodPut:
		app.updateEditorDocument(w, r, uid, vpath)
	default:
		app.writeEditorError(w, http.StatusMethodNotAllowed, "method not allowed")
	}
}

func (app *App) updateEditorDocument(w http.ResponseWriter, r *http.Request, uid, vpath string) {
	match, ok := normalizedIfMatch(r.Header.Get("If-Match"))
	if !ok {
		app.writeEditorError(w, http.StatusPreconditionRequired, "If-Match is required")
		return
	}
	var body struct {
		Content string  `json:"content"`
		Name    *string `json:"name"`
	}
	decoder := json.NewDecoder(io.LimitReader(r.Body, 2<<20))
	if err := decoder.Decode(&body); err != nil {
		app.writeEditorError(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	data := []byte(body.Content)
	if len(data) > editorFileLimit {
		app.writeEditorError(w, http.StatusRequestEntityTooLarge, "file is too large to edit")
		return
	}
	if !utf8.Valid(data) || bytes.ContainsRune(data, '\x00') {
		app.writeEditorError(w, http.StatusUnsupportedMediaType, "file is not UTF-8 text")
		return
	}
	name := filepath.Base(vpath)
	if body.Name != nil {
		name = *body.Name
	}
	if !editorName(name) {
		app.writeEditorError(w, http.StatusBadRequest, "invalid filename")
		return
	}
	dir := path.Dir(vpath)
	destination := path.Join(dir, name)
	if !strings.HasPrefix(destination, "/") {
		destination = "/" + destination
	}

	if app.quotaBytes > 0 {
		app.quotaMu.Lock()
		defer app.quotaMu.Unlock()
	}
	token, err := app.acquireLeases(r.Context(), uid, []string{vpath, destination}, 20*time.Second)
	if err != nil {
		if errors.Is(err, ErrLocked) {
			app.writeEditorError(w, http.StatusLocked, "file is busy")
		} else {
			app.writeEditorError(w, http.StatusInternalServerError, "could not lock file")
		}
		return
	}
	defer app.releaseLeases(r.Context(), token)

	current, status, err := readEditorDocument(app, uid, vpath)
	if err != nil {
		app.writeEditorError(w, status, err.Error())
		return
	}
	if current.ETag != match {
		app.writeEditorDocument(w, http.StatusPreconditionFailed, current)
		return
	}
	renamed := destination != vpath
	changed := !bytes.Equal([]byte(current.Content), data)
	if renamed {
		if _, err = app.Stat(r.Context(), uid, destination); err == nil {
			app.writeEditorError(w, http.StatusConflict, "a file with that name already exists")
			return
		} else if !os.IsNotExist(err) {
			app.writeEditorError(w, http.StatusInternalServerError, "could not check destination")
			return
		}
	}
	if !renamed && !changed {
		app.writeEditorDocument(w, http.StatusOK, current)
		return
	}
	if changed {
		if _, err = app.CreateVersion(r.Context(), uid, vpath); err != nil {
			app.writeEditorError(w, http.StatusInternalServerError, "could not create version")
			return
		}
	}
	if renamed {
		if err = app.renameUnlocked(r.Context(), uid, vpath, destination); err != nil {
			app.writeEditorError(w, http.StatusInternalServerError, "could not rename file")
			return
		}
	}
	if changed {
		writePath := vpath
		if renamed {
			writePath = destination
		}
		if err = app.writeStreamUnlocked(uid, writePath, bytes.NewReader(data)); err != nil {
			if renamed {
				_ = app.renameUnlocked(r.Context(), uid, destination, vpath)
			}
			if strings.Contains(err.Error(), "quota") {
				app.writeEditorError(w, http.StatusInsufficientStorage, "storage quota exceeded")
			} else {
				app.writeEditorError(w, http.StatusInternalServerError, "could not save file")
			}
			return
		}
	}
	if changed {
		app.logActivity(uid, destination, name, "edited")
	}
	if renamed {
		app.logActivity(uid, destination, name, "renamed")
	}
	app.writeEditorDocument(w, http.StatusOK, makeEditorDocument(destination, data))
}

func (app *App) handleMkdir(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	name := r.URL.Query().Get("name")
	if name == "" {
		http.Error(w, "missing name", 400)
		return
	}
	fullPath := vpath + "/" + name
	fullPath = app.cleanVPath(fullPath)
	if err := app.MkdirAll(r.Context(), uid, fullPath); err != nil {
		app.writeMutationError(w, err)
		return
	}
	app.logActivity(uid, fullPath, name, "created")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleTouch(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	name := r.URL.Query().Get("name")
	if name == "" {
		name = "Untitled.txt"
	}
	if !editorName(name) {
		http.Error(w, "invalid filename", http.StatusBadRequest)
		return
	}
	fullPath := vpath + "/" + name
	fullPath = app.cleanVPath(fullPath)
	if err := app.allowFilesMutation(fullPath); err != nil {
		app.writeMutationError(w, err)
		return
	}
	err := app.CreateFile(r.Context(), uid, fullPath)
	if errors.Is(err, os.ErrExist) {
		http.Error(w, "file already exists", http.StatusConflict)
		return
	}
	if err != nil {
		app.writeMutationError(w, err)
		return
	}
	app.logActivity(uid, fullPath, name, "created")
	app.writeJSON(w, map[string]any{"status": "ok", "path": fullPath, "name": name})
}

func (app *App) handleSymlink(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var input struct {
		Path   string `json:"path"`
		Target string `json:"target"`
	}
	if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
		http.Error(w, "invalid JSON", http.StatusBadRequest)
		return
	}
	if err := app.CreateSymlink(r.Context(), app.userID(r), input.Path, input.Target); err != nil {
		app.writeMutationError(w, err)
		return
	}
	app.logActivity(app.userID(r), input.Path, filepath.Base(input.Path), "created symbolic link")
	app.writeJSON(w, map[string]any{"status": "ok", "path": app.cleanVPath(input.Path), "target": input.Target})
}

func (app *App) handleRename(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" {
		http.Error(w, "missing src or dst", 400)
		return
	}
	if err := app.Rename(r.Context(), uid, src, dst); err != nil {
		app.writeMutationError(w, err)
		return
	}
	app.logActivity(uid, dst, filepath.Base(dst), "renamed")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleMove(w http.ResponseWriter, r *http.Request) {
	app.handleRename(w, r) // same operation
}

func (app *App) handleCopy(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" {
		http.Error(w, "missing src or dst", 400)
		return
	}
	if err := app.copyUserPath(r.Context(), uid, src, dst); err != nil {
		app.writeMutationError(w, err)
		return
	}
	app.logActivity(uid, dst, filepath.Base(dst), "copied")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) copyUserPath(ctx context.Context, uid, sourcePath, destinationPath string) error {
	source, err := app.resolveUserPath(ctx, uid, sourcePath, false)
	if err != nil {
		return err
	}
	destination, err := app.resolveUserPath(ctx, uid, destinationPath, true)
	if err != nil {
		return err
	}
	if err := app.allowFilesMutation(destination.OwnerPath); err != nil {
		return err
	}
	token, err := app.acquireLeases(ctx, destination.OwnerUID, []string{destination.OwnerPath}, 30*time.Second)
	if err != nil {
		return err
	}
	defer app.releaseLeases(ctx, token)
	sourceRoot, err := os.OpenRoot(app.userHome(source.OwnerUID))
	if err != nil {
		return err
	}
	defer sourceRoot.Close()
	destinationRoot, err := os.OpenRoot(app.userHome(destination.OwnerUID))
	if err != nil {
		return err
	}
	defer destinationRoot.Close()

	var copyEntry func(string, string) error
	copyEntry = func(src, dst string) error {
		info, err := sourceRoot.Lstat(src)
		if err != nil {
			return err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			target, err := sourceRoot.Readlink(src)
			if err != nil || filepath.IsAbs(target) {
				return errors.New("invalid symbolic link")
			}
			if err := destinationRoot.MkdirAll(filepath.Dir(dst), 0700); err != nil {
				return err
			}
			return destinationRoot.Symlink(target, dst)
		}
		if info.IsDir() {
			if err := destinationRoot.MkdirAll(dst, 0700); err != nil {
				return err
			}
			entries, err := fs.ReadDir(sourceRoot.FS(), src)
			if err != nil {
				return err
			}
			for _, entry := range entries {
				if err := copyEntry(path.Join(src, entry.Name()), path.Join(dst, entry.Name())); err != nil {
					return err
				}
			}
			return nil
		}
		input, err := sourceRoot.Open(src)
		if err != nil {
			return err
		}
		defer input.Close()
		if err := destinationRoot.MkdirAll(filepath.Dir(dst), 0700); err != nil {
			return err
		}
		temp := path.Join(filepath.ToSlash(filepath.Dir(dst)), ".dar-copy-"+leaseToken())
		output, err := destinationRoot.OpenFile(filepath.FromSlash(temp), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if err != nil {
			return err
		}
		defer destinationRoot.Remove(filepath.FromSlash(temp))
		if _, err := io.Copy(output, input); err != nil {
			_ = output.Close()
			return err
		}
		if err := output.Sync(); err != nil {
			_ = output.Close()
			return err
		}
		if err := output.Close(); err != nil {
			return err
		}
		return destinationRoot.Rename(filepath.FromSlash(temp), dst)
	}
	if err := copyEntry(strings.TrimPrefix(source.OwnerPath, "/"), strings.TrimPrefix(destination.OwnerPath, "/")); err != nil {
		return err
	}
	app.markSearchDirty()
	return nil
}

func (app *App) handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	vpath := app.cleanVPath(r.URL.Query().Get("path"))
	if err := app.allowFilesMutation(vpath); err != nil {
		app.writeMutationError(w, err)
		return
	}
	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, true)
	if err != nil {
		app.writeMutationError(w, err)
		return
	}
	if resolved.ShareID != "" {
		if err := app.Remove(r.Context(), uid, vpath); err != nil {
			app.writeMutationError(w, err)
			return
		}
		app.logActivity(uid, vpath, filepath.Base(vpath), "deleted shared item")
		app.writeJSON(w, map[string]string{"status": "ok"})
		return
	}
	trashPath := app.trashPathFromRequest(r)
	if err := app.allowFilesMutation(trashPath); err != nil {
		app.writeMutationError(w, err)
		return
	}
	if trashPath == "/Shared" || strings.HasPrefix(trashPath, "/Shared/") {
		http.Error(w, "shared paths cannot be used as trash", http.StatusBadRequest)
		return
	}
	// Soft delete: move to trash
	src := strings.TrimPrefix(vpath, "/")
	if src == "" {
		http.Error(w, "the Files root cannot be deleted", http.StatusBadRequest)
		return
	}
	trashName := newFileID() + "_" + filepath.Base(vpath)
	trashRoot := strings.TrimPrefix(trashPath, "/")
	dst := filepath.Join(trashRoot, trashName)
	token, err := app.acquireLeases(r.Context(), uid, []string{vpath, trashPath + "/" + trashName}, 20*time.Second)
	if err != nil {
		app.writeMutationError(w, err)
		return
	}
	defer app.releaseLeases(r.Context(), token)
	userRoot, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	defer userRoot.Close()
	if err := userRoot.MkdirAll(trashRoot, 0700); err != nil {
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	if err := userRoot.Rename(src, dst); err != nil {
		http.Error(w, "delete failed", 500)
		return
	}
	if err := app.putTrashRecord(r.Context(), uid, trashRecord{ID: trashName, OriginalPath: vpath, TrashPath: trashPath, DeletedAt: time.Now().Unix()}); err != nil {
		_ = userRoot.Rename(dst, src)
		http.Error(w, "delete failed", 500)
		return
	}
	app.logActivity(uid, vpath, filepath.Base(vpath), "deleted")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleRestore(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	trashName := r.URL.Query().Get("id")
	record, err := app.trashRecord(r.Context(), uid, trashName)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	trashRoot := strings.TrimPrefix(record.TrashPath, "/")
	src := filepath.Join(trashRoot, trashName)
	origPath := record.OriginalPath
	if origPath == "" {
		http.Error(w, "no original path", 400)
		return
	}
	if err := app.allowFilesMutation(origPath); err != nil {
		app.writeMutationError(w, err)
		return
	}
	dst := strings.TrimPrefix(origPath, "/")
	userRoot, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		http.Error(w, "restore failed", http.StatusInternalServerError)
		return
	}
	defer userRoot.Close()
	if err := userRoot.MkdirAll(filepath.Dir(dst), 0700); err != nil {
		http.Error(w, "restore failed", http.StatusInternalServerError)
		return
	}
	token, err := app.acquireLeases(r.Context(), uid, []string{record.TrashPath + "/" + trashName, origPath}, 20*time.Second)
	if err != nil {
		app.writeMutationError(w, err)
		return
	}
	defer app.releaseLeases(r.Context(), token)
	if err := userRoot.Rename(src, dst); err != nil {
		http.Error(w, "restore failed", 500)
		return
	}
	_ = app.deleteTrashRecord(r.Context(), uid, trashName)
	app.logActivity(uid, origPath, filepath.Base(origPath), "restored")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleDeletePermanent(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	trashName := r.URL.Query().Get("id")
	trashPath := app.trashPathFromRequest(r)
	trashRoot := strings.TrimPrefix(trashPath, "/")
	token, err := app.acquireLeases(r.Context(), uid, []string{trashPath + "/" + trashName}, 20*time.Second)
	if err != nil {
		app.writeMutationError(w, err)
		return
	}
	defer app.releaseLeases(r.Context(), token)
	userRoot, err := os.OpenRoot(app.userHome(uid))
	if err != nil {
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	defer userRoot.Close()
	if err := userRoot.RemoveAll(filepath.Join(trashRoot, trashName)); err != nil {
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	_ = app.deleteTrashRecord(r.Context(), uid, trashName)
	app.logActivity(uid, trashPath+"/"+trashName, trashName, "permanently deleted")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleFavorite(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	vpath = app.cleanVPath(vpath)
	favorited := false
	err := app.withConn(r.Context(), func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		var exists bool
		if err = sqlitex.Execute(c, `SELECT 1 FROM files_favorites WHERE user_id=? AND path=?`, &sqlitex.ExecOptions{Args: []any{uid, vpath}, ResultFunc: func(*sqlite.Stmt) error { exists = true; return nil }}); err != nil {
			return err
		}
		if exists {
			return sqlitex.Execute(c, `DELETE FROM files_favorites WHERE user_id=? AND path=?`, &sqlitex.ExecOptions{Args: []any{uid, vpath}})
		}
		favorited = true
		return sqlitex.Execute(c, `INSERT INTO files_favorites(user_id,path) VALUES(?,?)`, &sqlitex.ExecOptions{Args: []any{uid, vpath}})
	})
	if err != nil {
		http.Error(w, "favorite update failed", 500)
		return
	}
	action := "removed from favorites"
	if favorited {
		action = "added to favorites"
	}
	app.logActivity(uid, vpath, filepath.Base(vpath), action)
	app.writeJSON(w, map[string]any{"status": "ok", "favorited": favorited})
}

func (app *App) loadTags(uid string) map[string]Tag {
	m := map[string]Tag{}
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,name,color FROM files_tags WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			t := Tag{ID: s.ColumnText(0), Name: s.ColumnText(1), Color: s.ColumnText(2)}
			m[t.ID] = t
			return nil
		}})
	})
	return m
}
func (app *App) saveTags(uid string, m map[string]Tag) {
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM files_tags WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}}); err != nil {
			return err
		}
		for _, t := range m {
			if err = sqlitex.Execute(c, `INSERT INTO files_tags(user_id,id,name,color) VALUES(?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, t.ID, t.Name, t.Color}}); err != nil {
				return err
			}
		}
		return nil
	})
}
func (app *App) loadFileTags(uid string) map[string][]string {
	m := map[string][]string{}
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT path,tag_id FROM files_file_tags WHERE user_id=? ORDER BY path,tag_id`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { p := s.ColumnText(0); m[p] = append(m[p], s.ColumnText(1)); return nil }})
	})
	return m
}
func (app *App) saveFileTags(uid string, m map[string][]string) {
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM files_file_tags WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}}); err != nil {
			return err
		}
		for p, ids := range m {
			for _, id := range ids {
				if err = sqlitex.Execute(c, `INSERT INTO files_file_tags(user_id,path,tag_id) VALUES(?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(p), id}}); err != nil {
					return err
				}
			}
		}
		return nil
	})
}

func (app *App) handleTags(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	switch r.Method {
	case "GET":
		app.writeJSON(w, map[string]any{"tags": app.loadTags(uid)})
	case "POST":
		var t Tag
		json.NewDecoder(r.Body).Decode(&t)
		if t.Color == "" {
			t.Color = filesTheme.Pink
		}
		t.ID = newFileID()
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO files_tags(user_id,id,name,color) VALUES(?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, t.ID, t.Name, t.Color}})
		}); err != nil {
			http.Error(w, "tag creation failed", 500)
			return
		}
		app.logActivity(uid, "/tags/"+t.ID, t.Name, "created tag")
		app.writeJSON(w, t)
	case "DELETE":
		id := r.URL.Query().Get("id")
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) (err error) {
			end, err := sqlitex.ImmediateTransaction(c)
			if err != nil {
				return err
			}
			defer end(&err)
			if err = sqlitex.Execute(c, `DELETE FROM files_file_tags WHERE user_id=? AND tag_id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}}); err != nil {
				return err
			}
			return sqlitex.Execute(c, `DELETE FROM files_tags WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
		}); err != nil {
			http.Error(w, "tag deletion failed", 500)
			return
		}
		app.logActivity(uid, "/tags/"+id, "tag", "deleted tag")
		app.writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleTag(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var req struct {
		Path string `json:"path"`
		Tag  string `json:"tag"`
		Add  *bool  `json:"add"`
	}
	json.NewDecoder(r.Body).Decode(&req)
	req.Path = app.cleanVPath(req.Path)
	if req.Add == nil || *req.Add {
		_ = app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT OR IGNORE INTO files_file_tags(user_id,path,tag_id) VALUES(?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, req.Path, req.Tag}})
		})
	} else {
		_ = app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM files_file_tags WHERE user_id=? AND path=? AND tag_id=?`, &sqlitex.ExecOptions{Args: []any{uid, req.Path, req.Tag}})
		})
	}
	action := "tagged"
	if req.Add != nil && !*req.Add {
		action = "removed tag from"
	}
	app.logActivity(uid, req.Path, filepath.Base(req.Path), action)
	app.writeJSON(w, map[string]any{"status": "ok", "tags": app.loadFileTags(uid)[req.Path]})
}

// Update handleList to include tags and support tag filter
func (app *App) handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	q := strings.ToLower(r.URL.Query().Get("q"))
	typeFilter := strings.ToLower(r.URL.Query().Get("type"))
	folderScope := r.URL.Query().Get("folder")
	if q == "" && typeFilter == "" {
		app.writeJSON(w, FileResponse{Files: []FileEntry{}, Path: "/"})
		return
	}
	favs := app.loadFavorites(uid)
	fileTags := app.loadFileTags(uid)
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit <= 0 {
		limit = 50
	}
	if limit > 200 {
		limit = 200
	}
	offset, _ := strconv.Atoi(r.URL.Query().Get("offset"))
	if offset < 0 {
		offset = 0
	}
	matched := 0
	var results []FileEntry
	roots := []string{app.userHome(uid)}
	if folderScope != "" {
		roots = []string{app.realPath(uid, folderScope)}
	}
	for _, root := range roots {
		filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
			if err != nil || p == root {
				return nil
			}
			matchesName := q == "" || strings.Contains(strings.ToLower(info.Name()), q)
			matchesType := true
			if typeFilter != "" {
				switch typeFilter {
				case "folder":
					matchesType = info.IsDir()
				case "image":
					matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "image/")
				case "document":
					matchesType = !info.IsDir() && (strings.HasSuffix(info.Name(), ".txt") || strings.HasSuffix(info.Name(), ".md") || strings.HasSuffix(info.Name(), ".pdf"))
				case "video":
					matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "video/")
				case "audio":
					matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "audio/")
				}
			}
			if matchesName && matchesType {
				if matched < offset {
					matched++
					return nil
				}
				rel, _ := filepath.Rel(app.userHome(uid), p)
				if folderScope != "" {
					rel, _ = filepath.Rel(app.userHome(uid), p)
				}
				vp := "/" + filepath.ToSlash(rel)
				results = append(results, app.makeEntry(info, vp, favs, fileTags))
				matched++
			}
			if len(results) >= limit {
				return filepath.SkipAll
			}
			return nil
		})
	}
	if results == nil {
		results = []FileEntry{}
	}
	app.writeJSON(w, FileResponse{Files: results, Path: "/"})
}

func (app *App) saveVersion(uid, vpath string) {
	_, _ = app.CreateVersion(context.Background(), uid, vpath)
}

func (app *App) handleVersions(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	vers, _ := app.Versions(r.Context(), uid, vpath)
	if vers == nil {
		vers = []Version{}
	}
	app.writeJSON(w, map[string]any{"versions": vers})
}

func (app *App) handleVersion(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	vid := r.URL.Query().Get("id")
	switch r.Method {
	case "GET":
		data, err := app.ReadVersion(r.Context(), uid, vpath, vid)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		w.Write(data)
	case "POST":
		app.saveVersion(uid, vpath)
		data, err := app.ReadVersion(r.Context(), uid, vpath, vid)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		if err = app.WriteFile(r.Context(), uid, vpath, data); err != nil {
			http.Error(w, "restore failed", 500)
			return
		}
		app.logActivity(uid, vpath, filepath.Base(vpath), "restored version of")
		app.writeJSON(w, map[string]string{"status": "restored"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleUserShare(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case "POST":
		var req struct {
			Path       string `json:"path"`
			ToUser     string `json:"to_user"`
			Permission string `json:"permission"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "invalid JSON", http.StatusBadRequest)
			return
		}
		if req.Permission == "" {
			req.Permission = "read"
		}
		if req.Permission != "read" && req.Permission != "write" {
			http.Error(w, "permission must be read or write", http.StatusBadRequest)
			return
		}
		uid := app.userID(r)
		req.Path = app.cleanVPath(req.Path)
		if req.Path == "/" || req.Path == "/Shared" || strings.HasPrefix(req.Path, "/Shared/") || app.managedPathReadOnly(req.Path) {
			http.Error(w, "invalid shared path", http.StatusBadRequest)
			return
		}
		if _, err := app.Stat(r.Context(), uid, req.Path); err != nil {
			http.Error(w, "shared path not found", http.StatusNotFound)
			return
		}
		var recipient string
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `SELECT id FROM users WHERE id=? OR lower(email)=lower(?) LIMIT 1`, &sqlitex.ExecOptions{Args: []any{req.ToUser, req.ToUser}, ResultFunc: func(s *sqlite.Stmt) error {
				recipient = s.ColumnText(0)
				return nil
			}})
		}); err != nil {
			http.Error(w, "share failed", http.StatusInternalServerError)
			return
		}
		if recipient == "" {
			http.Error(w, "recipient not found", http.StatusNotFound)
			return
		}
		if recipient == uid {
			http.Error(w, "cannot share with yourself", http.StatusBadRequest)
			return
		}
		share := UserShare{ID: newFileID(),
			Path: req.Path, FromUser: uid, ToUser: recipient,
			Permission: req.Permission, Created: time.Now().Unix()}
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO files_user_shares(id,from_user,to_user,path,permission,created_at) VALUES(?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{share.ID, share.FromUser, share.ToUser, share.Path, share.Permission, share.Created}})
		}); err != nil {
			http.Error(w, "share failed", 500)
			return
		}
		app.markSearchDirty()
		app.logActivity(uid, share.Path, filepath.Base(share.Path), "shared")
		app.writeJSON(w, share)
	case "DELETE":
		uid := app.userID(r)
		id := r.URL.Query().Get("id")
		var removed UserShare
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) error {
			if err := sqlitex.Execute(c, `SELECT id,path,from_user,to_user,permission,created_at FROM files_user_shares WHERE id=? AND (from_user=? OR to_user=?)`, &sqlitex.ExecOptions{Args: []any{id, uid, uid}, ResultFunc: func(s *sqlite.Stmt) error {
				removed = UserShare{ID: s.ColumnText(0), Path: s.ColumnText(1), FromUser: s.ColumnText(2), ToUser: s.ColumnText(3), Permission: s.ColumnText(4), Created: s.ColumnInt64(5)}
				return nil
			}}); err != nil {
				return err
			}
			return sqlitex.Execute(c, `DELETE FROM files_user_shares WHERE id=? AND (from_user=? OR to_user=?)`, &sqlitex.ExecOptions{Args: []any{id, uid, uid}})
		}); err != nil {
			http.Error(w, "share deletion failed", 500)
			return
		}
		app.markSearchDirty()
		if removed.ID != "" {
			app.logActivityFor(uid, "user", "api", removed.Path, filepath.Base(removed.Path), "removed share for", []string{removed.FromUser, removed.ToUser})
		}
		app.writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleSharedWithMe(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	app.writeJSON(w, map[string]any{"shares": app.loadIncomingShares(uid)})
}

func (app *App) handleDiff(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	v1 := r.URL.Query().Get("v1")
	v2 := r.URL.Query().Get("v2")
	d1, err1 := app.ReadVersion(r.Context(), uid, vpath, v1)
	d2, err2 := app.ReadVersion(r.Context(), uid, vpath, v2)
	if err1 != nil || err2 != nil {
		http.Error(w, "version not found", http.StatusNotFound)
		return
	}
	lines1 := strings.Split(string(d1), "\n")
	lines2 := strings.Split(string(d2), "\n")
	var diff []string
	i, j := 0, 0
	for i < len(lines1) || j < len(lines2) {
		if i < len(lines1) && j < len(lines2) && lines1[i] == lines2[j] {
			diff = append(diff, "  "+lines1[i])
			i++
			j++
		} else if i < len(lines1) && (j >= len(lines2) || lines1[i] != lines2[j]) {
			diff = append(diff, "- "+lines1[i])
			i++
		} else {
			diff = append(diff, "+ "+lines2[j])
			j++
		}
	}
	app.writeJSON(w, map[string]any{"diff": strings.Join(diff, "\n")})
}

func (app *App) handleStats(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	app.writeJSON(w, map[string]any{"used": app.storageUsage(uid), "total": app.quotaBytes})
}

func (app *App) logActivity(uid, vpath, name, action string) {
	app.logActivityFor(uid, "user", "api", vpath, name, action, nil)
}

func (app *App) logActivityOrigin(uid, origin, vpath, name, action string) {
	app.logActivityFor(uid, "user", origin, vpath, name, action, nil)
}

func (app *App) logActivityFor(actorID, actorKind, origin, vpath, name, action string, explicitRecipients []string) {
	if app.registry == nil || (actorID == "" && len(explicitRecipients) == 0) {
		return
	}
	actorName := "User"
	if actorKind == "guest" {
		actorName = "Guest"
	}
	recipients := explicitRecipients
	if recipients == nil {
		recipients = []string{actorID}
	}
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		_ = sqlitex.Execute(c, `SELECT display_name FROM users WHERE id=?`, &sqlitex.ExecOptions{Args: []any{actorID}, ResultFunc: func(s *sqlite.Stmt) error {
			if strings.TrimSpace(s.ColumnText(0)) != "" {
				actorName = s.ColumnText(0)
			}
			return nil
		}})
		if explicitRecipients != nil {
			return nil
		}
		return sqlitex.Execute(c, `SELECT to_user,path FROM files_user_shares WHERE from_user=?`, &sqlitex.ExecOptions{Args: []any{actorID}, ResultFunc: func(s *sqlite.Stmt) error {
			sharedPath := app.cleanVPath(s.ColumnText(1))
			cleaned := app.cleanVPath(vpath)
			if cleaned == sharedPath || strings.HasPrefix(cleaned, sharedPath+"/") {
				recipients = append(recipients, s.ColumnText(0))
			}
			return nil
		}})
	})
	objectURL := "/files"
	for _, part := range strings.Split(strings.TrimPrefix(app.cleanVPath(vpath), "/"), "/") {
		if part != "" {
			objectURL += "/" + url.PathEscape(part)
		}
	}
	if err := app.registry.EnqueueActivity(context.Background(), apps.ActivityEvent{
		ActorID: actorID, ActorName: actorName, ActorKind: actorKind, Source: "files",
		Action: action, Summary: action + " " + name, ObjectType: "file", ObjectID: app.cleanVPath(vpath),
		ObjectName: name, ObjectURL: objectURL, Origin: origin, RecipientIDs: recipients,
	}); err != nil {
		log.Printf("record files activity: %v", err)
	}
}

type Share struct {
	Token        string `json:"token"`
	UserID       string `json:"-"`
	Path         string `json:"path"`
	PasswordSalt []byte `json:"-"`
	PasswordHash []byte `json:"-"`
	Expires      int64  `json:"expires,omitempty"`
	Drop         bool   `json:"drop"`
	Created      int64  `json:"created"`
}

func scanShare(s *sqlite.Stmt) Share {
	return Share{Token: s.ColumnText(0), UserID: s.ColumnText(1), Path: s.ColumnText(2), PasswordSalt: stmtBlob(s, 3), PasswordHash: stmtBlob(s, 4), Expires: s.ColumnInt64(5), Drop: s.ColumnInt64(6) != 0, Created: s.ColumnInt64(7)}
}

func stmtBlob(s *sqlite.Stmt, column int) []byte {
	b := make([]byte, s.ColumnLen(column))
	s.ColumnBytes(column, b)
	return b
}

func (app *App) listShares(ctx context.Context, uid string) ([]Share, error) {
	var out []Share
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT token,user_id,path,password_salt,password_hash,expires,drop_only,created_at FROM files_public_shares WHERE user_id=? ORDER BY created_at DESC`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { out = append(out, scanShare(s)); return nil }})
	})
	if out == nil {
		out = []Share{}
	}
	return out, err
}

func (app *App) shareByToken(ctx context.Context, token string) (Share, error) {
	var out Share
	err := app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT token,user_id,path,password_salt,password_hash,expires,drop_only,created_at FROM files_public_shares WHERE token=?`, &sqlitex.ExecOptions{Args: []any{token}, ResultFunc: func(s *sqlite.Stmt) error { out = scanShare(s); return nil }})
	})
	if err == nil && out.Token == "" {
		err = os.ErrNotExist
	}
	return out, err
}

func randomShareToken() (string, error) {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", err
	}
	return hex.EncodeToString(b[:]), nil
}

func newFileID() string { token, _ := randomShareToken(); return token }

func (app *App) passwordDigest(salt []byte, password string) ([]byte, error) {
	if len(app.sharePasswordKey) != 32 {
		return nil, errors.New("credential encryption keys are required for password-protected shares")
	}
	mac := hmac.New(sha256.New, app.sharePasswordKey)
	_, _ = mac.Write([]byte("dar-share-password-v1\x00"))
	_, _ = mac.Write(salt)
	_, _ = mac.Write([]byte(password))
	return mac.Sum(nil), nil
}

func (app *App) accessCookieValue(token string) string {
	mac := hmac.New(sha256.New, app.sharePasswordKey)
	_, _ = mac.Write([]byte("dar-share-access-v1\x00" + token))
	return hex.EncodeToString(mac.Sum(nil))
}

func (app *App) shareUnlocked(r *http.Request, share Share) bool {
	if len(share.PasswordHash) == 0 {
		return true
	}
	cookie, err := r.Cookie("dar_share_access")
	return err == nil && hmac.Equal([]byte(cookie.Value), []byte(app.accessCookieValue(share.Token)))
}

func shareExpired(share Share) bool { return share.Expires > 0 && share.Expires <= time.Now().Unix() }

func requestPeer(r *http.Request) string {
	host, _, err := net.SplitHostPort(strings.TrimSpace(r.RemoteAddr))
	if err == nil {
		return host
	}
	return strings.TrimSpace(r.RemoteAddr)
}

type Comment struct {
	ID        string `json:"id"`
	Path      string `json:"path"`
	Author    string `json:"author"`
	Body      string `json:"body"`
	ParentID  string `json:"parent_id,omitempty"`
	CreatedAt int64  `json:"created_at"`
}

func (app *App) loadComments(uid, vpath string) []Comment {
	var out []Comment
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,path,author,body,parent_id,created_at FROM files_comments WHERE user_id=? AND path=? ORDER BY created_at,id`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath)}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, Comment{ID: s.ColumnText(0), Path: s.ColumnText(1), Author: s.ColumnText(2), Body: s.ColumnText(3), ParentID: s.ColumnText(4), CreatedAt: s.ColumnInt64(5)})
			return nil
		}})
	})
	return out
}
func (app *App) saveComments(uid, vpath string, c []Comment) {
	_ = app.withConn(context.Background(), func(conn *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(conn)
		if err != nil {
			return err
		}
		defer end(&err)
		p := app.cleanVPath(vpath)
		if err = sqlitex.Execute(conn, `DELETE FROM files_comments WHERE user_id=? AND path=?`, &sqlitex.ExecOptions{Args: []any{uid, p}}); err != nil {
			return err
		}
		for _, x := range c {
			if err = sqlitex.Execute(conn, `INSERT INTO files_comments(user_id,path,id,author,body,parent_id,created_at) VALUES(?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{uid, p, x.ID, x.Author, x.Body, x.ParentID, x.CreatedAt}}); err != nil {
				return err
			}
		}
		return nil
	})
}

func (app *App) handleWebDAV(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := "/" + strings.TrimPrefix(r.URL.Path, "/webdav/")
	switch r.Method {
	case "PROPFIND":
		resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
		if err != nil {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
		if err != nil {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		defer root.Close()
		rootPath := strings.TrimPrefix(resolved.OwnerPath, "/")
		if rootPath == "" {
			rootPath = "."
		}
		// Basic PROPFIND response
		w.Header().Set("Content-Type", "application/xml; charset=utf-8")
		w.Header().Set("DAV", "1,2")
		w.WriteHeader(207)
		entries, _ := fs.ReadDir(root.FS(), rootPath)
		xml := `<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:">`
		for _, e := range entries {
			info, _ := e.Info()
			href := "/webdav/" + strings.TrimPrefix(vpath, "/") + "/" + url.PathEscape(e.Name())
			if vpath == "/" {
				href = "/webdav/" + url.PathEscape(e.Name())
			}
			xml += `<d:response><d:href>` + html.EscapeString(href) + `</d:href><d:propstat><d:prop>`
			if e.IsDir() {
				xml += `<d:resourcetype><d:collection/></d:resourcetype>`
			}
			xml += `<d:getcontentlength>` + fmt.Sprintf("%d", info.Size()) + `</d:getcontentlength>`
			xml += `<d:getlastmodified>` + info.ModTime().Format(time.RFC3339) + `</d:getlastmodified>`
			xml += `</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`
		}
		xml += `</d:multistatus>`
		w.Write([]byte(xml))
	case "MKCOL":
		if err := app.MkdirAll(r.Context(), uid, vpath); err != nil {
			app.writeMutationError(w, err)
			return
		}
		app.logActivityOrigin(uid, "dav", vpath, filepath.Base(vpath), "created")
		w.WriteHeader(201)
	case "PUT":
		data, err := io.ReadAll(io.LimitReader(r.Body, 129<<20))
		if err != nil {
			http.Error(w, "invalid body", 400)
			return
		}
		if err = app.WriteFile(r.Context(), uid, vpath, data); err != nil {
			app.writeMutationError(w, err)
			return
		}
		app.logActivityOrigin(uid, "dav", vpath, filepath.Base(vpath), "uploaded")
		w.WriteHeader(201)
	case "DELETE":
		if err := app.Remove(r.Context(), uid, vpath); err != nil {
			app.writeMutationError(w, err)
			return
		}
		app.logActivityOrigin(uid, "dav", vpath, filepath.Base(vpath), "deleted")
		w.WriteHeader(204)
	case "GET":
		resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
		if err != nil {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
		if err != nil {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		defer root.Close()
		rootPath := strings.TrimPrefix(resolved.OwnerPath, "/")
		if rootPath == "" {
			rootPath = "."
		}
		info, err := root.Stat(rootPath)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		if info.IsDir() {
			entries, _ := fs.ReadDir(root.FS(), rootPath)
			htmlPage := standalonePageHead(html.EscapeString(vpath), theme.FromContext(r.Context())) + `<main><h1>` + html.EscapeString(vpath) + `</h1><ul>`
			for _, e := range entries {
				href := strings.TrimRight(r.URL.Path, "/") + "/" + url.PathEscape(e.Name())
				htmlPage += `<li><a href="` + html.EscapeString(href) + `">` + html.EscapeString(e.Name()) + `</a></li>`
			}
			htmlPage += `</ul></main></body></html>`
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			w.Header().Set("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'")
			w.Write([]byte(htmlPage))
			return
		}
		file, err := root.Open(rootPath)
		if err != nil {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		defer file.Close()
		http.ServeContent(w, r, info.Name(), info.ModTime(), file)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleNextcloudWebDAV(w http.ResponseWriter, r *http.Request) {
	rest := strings.TrimPrefix(r.URL.Path, "/remote.php/dav/files/")
	parts := strings.SplitN(rest, "/", 2)
	if len(parts) == 2 {
		r.URL.Path = "/webdav/" + parts[1]
	} else {
		r.URL.Path = "/webdav/"
	}
	app.handleWebDAV(w, r)
}

func (app *App) handleComments(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	switch r.Method {
	case "GET":
		comments := app.loadComments(uid, vpath)
		if comments == nil {
			comments = []Comment{}
		}
		app.writeJSON(w, map[string]any{"comments": comments})
	case "POST":
		var c Comment
		json.NewDecoder(r.Body).Decode(&c)
		c.ID = newFileID()
		c.Path = vpath
		c.Author = uid
		c.CreatedAt = time.Now().Unix()
		comments := app.loadComments(uid, vpath)
		comments = append(comments, c)
		app.saveComments(uid, vpath, comments)
		app.logActivity(uid, vpath, filepath.Base(vpath), "commented on")
		app.writeJSON(w, c)
	case "DELETE":
		id := r.URL.Query().Get("id")
		comments := app.loadComments(uid, vpath)
		for i, c := range comments {
			if c.ID == id {
				comments = append(comments[:i], comments[i+1:]...)
				break
			}
		}
		app.saveComments(uid, vpath, comments)
		app.logActivity(uid, vpath, filepath.Base(vpath), "deleted comment from")
		app.writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleShare(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPost:
		uid := app.userID(r)
		var req struct {
			Path     string `json:"path"`
			Password string `json:"password"`
			Expires  int64  `json:"expires"`
			Drop     bool   `json:"drop"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil || strings.TrimSpace(req.Path) == "" {
			http.Error(w, "invalid share", http.StatusBadRequest)
			return
		}
		if req.Expires > 0 && req.Expires <= time.Now().Unix() {
			http.Error(w, "expiry must be in the future", http.StatusBadRequest)
			return
		}
		req.Path = app.cleanVPath(req.Path)
		if req.Path == "/Shared" || strings.HasPrefix(req.Path, "/Shared/") || app.managedPathReadOnly(req.Path) {
			http.Error(w, "this path cannot be publicly shared", http.StatusBadRequest)
			return
		}
		if _, err := app.Stat(r.Context(), uid, req.Path); err != nil {
			http.Error(w, "shared path not found", http.StatusNotFound)
			return
		}
		token, err := randomShareToken()
		if err != nil {
			http.Error(w, "share creation failed", http.StatusInternalServerError)
			return
		}
		share := Share{Token: token, UserID: uid, Path: req.Path, Drop: req.Drop, Expires: req.Expires, Created: time.Now().Unix()}
		if req.Password != "" {
			share.PasswordSalt = make([]byte, 16)
			if _, err := rand.Read(share.PasswordSalt); err != nil {
				http.Error(w, "share creation failed", 500)
				return
			}
			share.PasswordHash, err = app.passwordDigest(share.PasswordSalt, req.Password)
			if err != nil {
				log.Printf("create password-protected share: %v", err)
				http.Error(w, "password-protected shares are unavailable", http.StatusServiceUnavailable)
				return
			}
		}
		drop := 0
		if share.Drop {
			drop = 1
		}
		err = app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO files_public_shares(token,user_id,path,password_salt,password_hash,expires,drop_only,created_at) VALUES(?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{share.Token, share.UserID, share.Path, share.PasswordSalt, share.PasswordHash, share.Expires, drop, share.Created}})
		})
		if err != nil {
			http.Error(w, "share creation failed", http.StatusInternalServerError)
			return
		}
		app.logActivity(uid, share.Path, filepath.Base(share.Path), "created public share for")
		app.writeJSON(w, share)
	case http.MethodDelete:
		uid := app.userID(r)
		token := r.URL.Query().Get("token")
		if err := app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM files_public_shares WHERE user_id=? AND token=?`, &sqlitex.ExecOptions{Args: []any{uid, token}})
		}); err != nil {
			http.Error(w, "share deletion failed", 500)
			return
		}
		app.logActivity(uid, "/shares/"+token, "public share", "revoked")
		app.writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) servePublicShare(w http.ResponseWriter, r *http.Request, token string) {
	app.servePublicSharePath(w, r, token, "")
}

func (app *App) servePublicSharePath(w http.ResponseWriter, r *http.Request, token, relativePath string) {
	share, err := app.shareByToken(r.Context(), token)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if shareExpired(share) {
		http.Error(w, "share expired", http.StatusGone)
		return
	}
	w.Header().Set("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'")
	appearance := app.publicShareAppearance(r.Context(), share, token)
	if !app.shareUnlocked(r, share) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte(standalonePageHead("Unlock share", appearance) + `<main><h1>Unlock share</h1><form method="POST" action="/s/` + html.EscapeString(token) + `/unlock"><label>Password <input name="password" type="password" required></label><button type="submit">Unlock</button></form></main></body></html>`))
		return
	}
	if share.Drop {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte(standalonePageHead("File Drop", appearance) + `<main><h1>File Drop</h1><p>Upload files to ` + html.EscapeString(filepath.Base(share.Path)) + `</p><form method="POST" action="/s/` + html.EscapeString(token) + `/upload" enctype="multipart/form-data"><input type="file" name="file" multiple required><button type="submit">Upload</button></form></main></body></html>`))
		return
	}
	relativePath = strings.TrimSpace(strings.ReplaceAll(relativePath, `\`, "/"))
	for _, segment := range strings.Split(relativePath, "/") {
		if segment == ".." {
			http.NotFound(w, r)
			return
		}
	}
	relativePath = strings.TrimPrefix(path.Clean("/"+relativePath), "/")
	root, err := os.OpenRoot(app.userHome(share.UserID))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer root.Close()
	target := path.Join(strings.TrimPrefix(app.cleanVPath(share.Path), "/"), relativePath)
	info, err := root.Stat(filepath.FromSlash(target))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if !info.IsDir() {
		file, err := root.Open(filepath.FromSlash(target))
		if err != nil {
			http.NotFound(w, r)
			return
		}
		defer file.Close()
		http.ServeContent(w, r, info.Name(), info.ModTime(), file)
		return
	}
	entries, err := fs.ReadDir(root.FS(), filepath.FromSlash(target))
	if err != nil {
		http.Error(w, "share unavailable", http.StatusInternalServerError)
		return
	}
	var list strings.Builder
	list.WriteString(`<main><h1>`)
	list.WriteString(html.EscapeString(info.Name()))
	list.WriteString(`</h1><ul>`)
	if strings.Trim(relativePath, "/") != "" {
		list.WriteString(`<li><a href="/s/`)
		list.WriteString(html.EscapeString(token))
		parent := path.Dir("/" + strings.Trim(relativePath, "/"))
		if parent != "/" {
			list.WriteString(publicShareURLPath(parent))
		}
		list.WriteString(`">Parent folder</a></li>`)
	}
	for _, entry := range entries {
		name := entry.Name()
		href := path.Join("/", strings.Trim(relativePath, "/"), name)
		list.WriteString(`<li><a href="/s/`)
		list.WriteString(html.EscapeString(token))
		list.WriteString(publicShareURLPath(href))
		list.WriteString(`">`)
		list.WriteString(html.EscapeString(name))
		childInfo, childErr := root.Stat(filepath.FromSlash(path.Join(target, name)))
		if childErr == nil && childInfo.IsDir() {
			list.WriteString("/")
		}
		list.WriteString(`</a></li>`)
	}
	list.WriteString(`</ul></main></body></html>`)
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	_, _ = w.Write([]byte(standalonePageHead("Shared files", appearance) + list.String()))
}

func publicShareURLPath(value string) string {
	segments := strings.Split(strings.Trim(value, "/"), "/")
	var encoded strings.Builder
	for _, segment := range segments {
		if segment == "" {
			continue
		}
		encoded.WriteByte('/')
		encoded.WriteString(url.PathEscape(segment))
	}
	return encoded.String()
}

func (app *App) unlockPublicShare(w http.ResponseWriter, r *http.Request, token string) {
	share, err := app.shareByToken(r.Context(), token)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if shareExpired(share) {
		http.Error(w, "share expired", http.StatusGone)
		return
	}
	if !app.unlockLimiter.Allow(token + "\x00" + requestPeer(r)) {
		w.Header().Set("Retry-After", "60")
		http.Error(w, "too many unlock attempts", http.StatusTooManyRequests)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "invalid form", 400)
		return
	}
	digest, err := app.passwordDigest(share.PasswordSalt, r.Form.Get("password"))
	if err != nil || !hmac.Equal(digest, share.PasswordHash) {
		http.Error(w, "invalid password", http.StatusUnauthorized)
		return
	}
	maxAge := 3600
	if share.Expires > 0 && int64(maxAge) > share.Expires-time.Now().Unix() {
		maxAge = int(share.Expires - time.Now().Unix())
	}
	http.SetCookie(w, &http.Cookie{Name: "dar_share_access", Value: app.accessCookieValue(token), Path: "/s/" + token, MaxAge: maxAge, HttpOnly: true, Secure: app.secureCookies, SameSite: http.SameSiteStrictMode})
	http.Redirect(w, r, "/s/"+token, http.StatusSeeOther)
}

func (app *App) handlePublicShareUpload(w http.ResponseWriter, r *http.Request, token string) {
	share, err := app.shareByToken(r.Context(), token)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if shareExpired(share) {
		http.Error(w, "share expired", http.StatusGone)
		return
	}
	if !share.Drop || !app.shareUnlocked(r, share) {
		http.Error(w, "share upload denied", http.StatusForbidden)
		return
	}
	if !app.uploadLimiter.Allow(token + "\x00" + requestPeer(r)) {
		w.Header().Set("Retry-After", "60")
		http.Error(w, "too many uploads", http.StatusTooManyRequests)
		return
	}
	mr, err := r.MultipartReader()
	if err != nil {
		http.Error(w, "invalid multipart upload", 400)
		return
	}
	for {
		part, err := mr.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			http.Error(w, "invalid multipart upload", 400)
			return
		}
		name, nameErr := uploadedFilename(part)
		if nameErr != nil {
			_ = part.Close()
			http.Error(w, "invalid filename", http.StatusBadRequest)
			return
		}
		if name != "" {
			if err := app.saveUploadedStream(r.Context(), share.UserID, share.Path, name, part); err != nil {
				_ = part.Close()
				http.Error(w, "upload failed", 400)
				return
			}
			app.logActivityFor("", "guest", "public", app.cleanVPath(share.Path+"/"+name), name, "uploaded", []string{share.UserID})
		}
		_ = part.Close()
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'")
	appearance := app.publicShareAppearance(r.Context(), share, token)
	_, _ = w.Write([]byte(standalonePageHead("Files uploaded", appearance) + `<main><h1>Files uploaded</h1><a href="/s/` + html.EscapeString(token) + `">Upload more</a></main></body></html>`))
}

func (app *App) publicShareAppearance(ctx context.Context, share Share, token string) theme.Resolved {
	if app.appearance == nil {
		return theme.FromContext(context.Background())
	}
	resolved, err := app.appearance.Resolve(ctx, share.UserID)
	if err != nil {
		return theme.FromContext(context.Background())
	}
	if resolved.HasWallpaper {
		resolved.PublicWallpaperURL = "/s/" + token + "/appearance/wallpaper"
	}
	return resolved
}

func (app *App) servePublicShareWallpaper(w http.ResponseWriter, r *http.Request, token string) {
	share, err := app.shareByToken(r.Context(), token)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	if shareExpired(share) {
		http.Error(w, "share expired", http.StatusGone)
		return
	}
	if app.appearance == nil {
		http.NotFound(w, r)
		return
	}
	app.appearance.ServeWallpaper(w, r, share.UserID)
}

func standalonePageHead(title string, appearance theme.Resolved) string {
	wallpaperURL := appearance.PublicWallpaperURL
	return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="theme-color" content="` + appearance.Active.Surface + `"><meta name="color-scheme" content="light dark"><title>` + title + `</title><style>` + theme.StandaloneAppearanceCSS(appearance, wallpaperURL) + `</style></head><body>`
}

func (app *App) handleShared(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	shares, err := app.listShares(r.Context(), uid)
	if err != nil {
		http.Error(w, "could not list shares", 500)
		return
	}
	app.writeJSON(w, map[string]any{"shares": shares})
}

func (app *App) loadUserShares(uid string) []UserShare {
	var out []UserShare
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,path,from_user,to_user,permission,created_at FROM files_user_shares WHERE from_user=? OR to_user=? ORDER BY created_at DESC,id`, &sqlitex.ExecOptions{Args: []any{uid, uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, UserShare{ID: s.ColumnText(0), Path: s.ColumnText(1), FromUser: s.ColumnText(2), ToUser: s.ColumnText(3), Permission: s.ColumnText(4), Created: s.ColumnInt64(5)})
			return nil
		}})
	})
	if out == nil {
		out = []UserShare{}
	}
	return out
}

func (app *App) loadIncomingShares(uid string) []UserShare {
	var out []UserShare
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,path,from_user,to_user,permission,created_at FROM files_user_shares WHERE to_user=? ORDER BY created_at DESC,id`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, UserShare{ID: s.ColumnText(0), Path: s.ColumnText(1), FromUser: s.ColumnText(2), ToUser: s.ColumnText(3), Permission: s.ColumnText(4), Created: s.ColumnInt64(5)})
			return nil
		}})
	})
	if out == nil {
		out = []UserShare{}
	}
	return out
}
func (app *App) saveUserShares(uid string, s []UserShare) {
	_ = app.withConn(context.Background(), func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM files_user_shares WHERE from_user=? OR to_user=?`, &sqlitex.ExecOptions{Args: []any{uid, uid}}); err != nil {
			return err
		}
		for _, x := range s {
			if err = sqlitex.Execute(c, `INSERT INTO files_user_shares(id,from_user,to_user,path,permission,created_at) VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET from_user=excluded.from_user,to_user=excluded.to_user,path=excluded.path,permission=excluded.permission,created_at=excluded.created_at`, &sqlitex.ExecOptions{Args: []any{x.ID, x.FromUser, x.ToUser, x.Path, x.Permission, x.Created}}); err != nil {
				return err
			}
		}
		return nil
	})
	app.markSearchDirty()
}

func (app *App) handleLock(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	var locks map[string]string
	locks = map[string]string{}
	_ = app.withConn(r.Context(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT path,owner FROM files_locks WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error { locks[s.ColumnText(0)] = s.ColumnText(1); return nil }})
	})
	switch r.Method {
	case "GET":
		owner, locked := locks[vpath]
		app.writeJSON(w, map[string]any{"locked": locked, "owner": owner})
	case "POST":
		_ = app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO files_locks(user_id,path,owner) VALUES(?,?,?) ON CONFLICT(user_id,path) DO UPDATE SET owner=excluded.owner`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath), uid}})
		})
		app.logActivity(uid, vpath, filepath.Base(vpath), "locked")
		app.writeJSON(w, map[string]any{"status": "locked"})
	case "DELETE":
		_ = app.withConn(r.Context(), func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM files_locks WHERE user_id=? AND path=?`, &sqlitex.ExecOptions{Args: []any{uid, app.cleanVPath(vpath)}})
		})
		app.logActivity(uid, vpath, filepath.Base(vpath), "unlocked")
		app.writeJSON(w, map[string]any{"status": "unlocked"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleHealth(w http.ResponseWriter, r *http.Request) {
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

func (app *App) handleThumbnail(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "/Shared" {
		shares := app.loadIncomingShares(uid)
		var files []FileEntry
		for _, s := range shares {
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: "/Shared/" + s.ID, IsDir: false, UpdatedAt: s.Created})
		}
		if files == nil {
			files = []FileEntry{}
		}
		app.writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	defer root.Close()
	src, err := root.Open(strings.TrimPrefix(resolved.OwnerPath, "/"))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	defer src.Close()
	config, _, err := image.DecodeConfig(src)
	if err != nil || config.Width < 1 || config.Height < 1 || config.Width > 20_000 || config.Height > 20_000 || int64(config.Width)*int64(config.Height) > 40_000_000 {
		http.Error(w, "image dimensions are unsupported", http.StatusRequestEntityTooLarge)
		return
	}
	if _, err := src.Seek(0, io.SeekStart); err != nil {
		http.Error(w, "not an image", http.StatusUnsupportedMediaType)
		return
	}
	img, _, err := image.Decode(src)
	if err != nil {
		http.Error(w, "not an image", 415)
		return
	}
	b := img.Bounds()
	maxW := 200
	ratio := float64(maxW) / float64(b.Dx())
	newH := int(float64(b.Dy()) * ratio)
	if b.Dx() <= maxW {
		newH = b.Dy()
		maxW = b.Dx()
	}
	scaled := image.NewRGBA(image.Rect(0, 0, maxW, newH))
	for y := 0; y < newH; y++ {
		for x := 0; x < maxW; x++ {
			sx := int(float64(x) / ratio)
			if b.Dx() <= maxW {
				sx = x
			}
			sy := int(float64(y) / ratio)
			if b.Dx() <= maxW {
				sy = y
			}
			scaled.Set(x, y, img.At(sx, sy))
		}
	}
	w.Header().Set("Content-Type", "image/jpeg")
	if err := jpeg.Encode(w, scaled, &jpeg.Options{Quality: 70}); err != nil {
		log.Printf("encode thumbnail: %v", err)
	}
}

func (app *App) handlePreview(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "/Shared" {
		shares := app.loadIncomingShares(uid)
		var files []FileEntry
		for _, s := range shares {
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: "/Shared/" + s.ID, IsDir: false, UpdatedAt: s.Created})
		}
		if files == nil {
			files = []FileEntry{}
		}
		app.writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	resolved, err := app.resolveUserPath(r.Context(), uid, vpath, false)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	root, err := os.OpenRoot(app.userHome(resolved.OwnerUID))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	defer root.Close()
	data, err := root.ReadFile(strings.TrimPrefix(resolved.OwnerPath, "/"))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	mtype := mime.TypeByExtension(filepath.Ext(vpath))
	content := string(data)
	if len(content) > 512*1024 {
		content = content[:512*1024]
	}
	app.writeJSON(w, map[string]string{"type": mtype, "content": content, "name": filepath.Base(vpath)})
}

func (app *App) addTaggedFiles(uid, root, prefix string, ft map[string][]string, tagID string, favs map[string]bool, files *[]FileEntry) {
	filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
		if err != nil || p == root {
			return nil
		}
		rel, _ := filepath.Rel(root, p)
		vp := prefix + "/" + filepath.ToSlash(rel)
		for _, t := range ft[vp] {
			if t == tagID {
				*files = append(*files, app.makeEntry(info, vp, favs, ft))
				if info.IsDir() {
					return filepath.SkipDir
				}
				return nil
			}
		}
		return nil
	})
}
