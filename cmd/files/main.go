// Files daemon — disk-based storage with trash, favorites, text files, copy.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"image"
	"image/jpeg"
	_ "image/png"
	"io"
	"log"
	"mime"
	"mime/multipart"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

var (
	port     = flag.Int("port", 8082, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/list", handleList)
	mux.HandleFunc("/upload", handleUpload)
	mux.HandleFunc("/download", handleDownload)
	mux.HandleFunc("/mkdir", handleMkdir)
	mux.HandleFunc("/touch", handleTouch)
	mux.HandleFunc("/rename", handleRename)
	mux.HandleFunc("/move", handleMove)
	mux.HandleFunc("/copy", handleCopy)
	mux.HandleFunc("/delete", handleDelete)
	mux.HandleFunc("/restore", handleRestore)
	mux.HandleFunc("/delete-permanent", handleDeletePermanent)
	mux.HandleFunc("/favorite", handleFavorite)
	mux.HandleFunc("/search", handleSearch)
	mux.HandleFunc("/tags", handleTags)
	mux.HandleFunc("/tag", handleTag)
	mux.HandleFunc("/thumb", handleThumbnail)
	mux.HandleFunc("/preview", handlePreview)
	mux.HandleFunc("/versions", handleVersions)
	mux.HandleFunc("/version", handleVersion)
	mux.HandleFunc("/diff", handleDiff)
	mux.HandleFunc("/stats", handleStats)
	mux.HandleFunc("/share", handleShare)
	mux.HandleFunc("/shared", handleShared)
	mux.HandleFunc("/webdav/", handleWebDAV)
	mux.HandleFunc("/comments", handleComments)
	mux.HandleFunc("/lock", handleLock)
	mux.HandleFunc("/activity", handleActivity)
	mux.HandleFunc("/share-user", handleUserShare)
	mux.HandleFunc("/external-storage", handleExternal)
	mux.HandleFunc("/shared-with-me", handleSharedWithMe)
	mux.HandleFunc("/health", handleHealth)
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[files] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type FileEntry struct {
	Name      string `json:"name"`
	Path      string `json:"path"`
	Size      int64  `json:"size"`
	MimeType  string `json:"mime_type"`
	IsDir     bool   `json:"is_dir"`
	UpdatedAt int64  `json:"updated_at"`
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

func userHome(uid string) string {
	d := filepath.Join(*dataRoot, uid, "home")
	os.MkdirAll(d, 0755)
	return d
}

func filesDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "files")
	os.MkdirAll(d, 0755)
	return d
}

func trashDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "trash")
	os.MkdirAll(d, 0755)
	return d
}

func realPath(uid, vpath string) string {
	// /files/... → personal files, /... → user home
	if strings.HasPrefix(vpath, "/files/") || vpath == "/files" {
		return filepath.Join(filesDir(uid), filepath.Clean(strings.TrimPrefix(vpath, "/files")))
	}
	return filepath.Join(userHome(uid), filepath.Clean("/"+vpath))
}

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func loadFavorites(uid string) map[string]bool {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "favorites.json"))
	var m map[string]bool
	json.Unmarshal(data, &m)
	if m == nil { m = map[string]bool{} }
	return m
}
func saveFavorites(uid string, m map[string]bool) {
	data, _ := json.Marshal(m)
	os.MkdirAll(filepath.Join(*dataRoot, uid), 0755)
	os.WriteFile(filepath.Join(*dataRoot, uid, "favorites.json"), data, 0644)
}

func handleList(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	filter := r.URL.Query().Get("filter")
	favs := loadFavorites(uid)

	// Trash view
	if vpath == "/trash" {
		entries, _ := os.ReadDir(trashDir(uid))
		var files []FileEntry
		for _, e := range entries {
			info, _ := e.Info()
			origPath, _ := os.ReadFile(filepath.Join(trashDir(uid), e.Name()+".orig"))
			files = append(files, FileEntry{
				Name: e.Name(), Path: string(origPath), Size: info.Size(),
				IsDir: e.IsDir(), UpdatedAt: info.ModTime().Unix(), TrashPath: e.Name(),
			})
		}
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		writeJSON(w, FileResponse{Files: files, Path: "/trash"})
		return
	}

	// Favorites filter
	if filter == "favorites" {
		root := userHome(uid)
		filesRoot := filesDir(uid)
		var files []FileEntry
		for _, rt := range []struct{root,prefix string}{{root, ""}, {filesRoot, "/files"}} {
			filepath.Walk(rt.root, func(p string, info os.FileInfo, err error) error {
				if err != nil || p == rt.root { return nil }
				rel, _ := filepath.Rel(rt.root, p)
				vp := rt.prefix + "/" + filepath.ToSlash(rel)
				if info.IsDir() && favs[vp] {
					files = append(files, makeEntry(info, vp, favs, loadFileTags(uid)))
					return filepath.SkipDir
				}
				if favs[vp] {
					files = append(files, makeEntry(info, vp, favs, loadFileTags(uid)))
				}
				return nil
			})
		}
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		writeJSON(w, FileResponse{Files: files, Path: "/"})
		return
	}

		// Tag filter
	if filter == "tag" {
		tagID := r.URL.Query().Get("tag")
		ft := loadFileTags(uid)
		var files []FileEntry
		addTaggedFiles(uid, userHome(uid), "", ft, tagID, favs, &files)
		addTaggedFiles(uid, filesDir(uid), "/files", ft, tagID, favs, &files)
		sort.Slice(files, func(i, j int) bool { return files[i].UpdatedAt > files[j].UpdatedAt })
		writeJSON(w, FileResponse{Files: files, Path: "/"})
		return
	}

	if vpath == "/Shared" {
		shares := loadUserShares(uid)
		var files []FileEntry
		for _, s := range shares {
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: s.Path, IsDir: false, UpdatedAt: s.Created})
		}
		if files == nil { files = []FileEntry{} }
		writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	rp := realPath(uid, vpath)
	entries, _ := os.ReadDir(rp)
	var files []FileEntry
	for _, e := range entries {
		info, _ := e.Info()
		childPath := vpath + "/" + e.Name()
		if vpath == "/" || vpath == "" { childPath = "/" + e.Name() }
		files = append(files, makeEntry(info, childPath, favs, loadFileTags(uid)))
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir { return files[i].IsDir }
		return strings.ToLower(files[i].Name) < strings.ToLower(files[j].Name)
	})
	if (vpath == "/" || vpath == "") && len(loadUserShares(uid)) > 0 {
		hasShared := false
		for _, f := range files { if f.Name == "Shared" { hasShared = true; break } }
		if !hasShared {
			files = append([]FileEntry{{Name: "Shared", Path: "/Shared", IsDir: true, UpdatedAt: time.Now().Unix()}}, files...)
		}
	}
	if files == nil { files = []FileEntry{} }
	writeJSON(w, FileResponse{Files: files, Path: vpath})
}

func makeEntry(info os.FileInfo, path string, favs map[string]bool, ftags map[string][]string) FileEntry {
	mtype := ""
	if !info.IsDir() { mtype = mime.TypeByExtension(filepath.Ext(info.Name())) }
	return FileEntry{
		Name: info.Name(), Path: path, Size: info.Size(), MimeType: mtype,
		IsDir: info.IsDir(), UpdatedAt: info.ModTime().Unix(), Favorited: favs[path],
		Tags: ftags[path],
	}
}

func handleUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "" || vpath == "/" { vpath = "/files" }
	r.ParseMultipartForm(128 << 20)
	// Support multiple files
	if mr := r.MultipartForm; mr != nil {
		for _, fhs := range mr.File {
			for _, fh := range fhs {
				saveUploadedFile(uid, vpath, fh)
			}
		}
	}
	writeJSON(w, map[string]string{"status": "ok"})
}

func saveUploadedFile(uid, vpath string, fh *multipart.FileHeader) {
	file, _ := fh.Open()
	if file == nil { return }
	defer file.Close()
	name := fh.Filename
	fullPath := vpath + "/" + name
	rp := realPath(uid, fullPath)
	os.MkdirAll(filepath.Dir(rp), 0755)
	// Save version if file exists
	if _, err := os.Stat(rp); err == nil { saveVersion(uid, fullPath) }
	dst, err := os.Create(rp)
	if err != nil { return }
	defer dst.Close()
	io.Copy(dst, file)
	logActivity(uid, fullPath, name, "uploaded")
}

func handleDownload(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	http.ServeFile(w, r, realPath(uid, vpath))
}

func handleMkdir(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	name := r.URL.Query().Get("name")
	if name == "" { http.Error(w, "missing name", 400); return }
	fullPath := vpath + "/" + name
	os.MkdirAll(realPath(uid, fullPath), 0755)
	logActivity(uid, fullPath, name, "created")
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleTouch(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	name := r.URL.Query().Get("name")
	if name == "" { name = "Untitled.txt" }
	fullPath := vpath + "/" + name
	rp := realPath(uid, fullPath)
	os.MkdirAll(filepath.Dir(rp), 0755)
	if _, err := os.Stat(rp); err == nil { saveVersion(uid, fullPath) }
	os.WriteFile(rp, []byte(""), 0644)
	logActivity(uid, fullPath, name, "created")
	writeJSON(w, map[string]any{"status": "ok", "path": fullPath, "name": name})
}

func handleRename(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" { http.Error(w, "missing src or dst", 400); return }
	os.Rename(realPath(uid, src), realPath(uid, dst))
	logActivity(uid, dst, filepath.Base(dst), "renamed")
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleMove(w http.ResponseWriter, r *http.Request) {
	handleRename(w, r) // same operation
}

func handleCopy(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" { http.Error(w, "missing src or dst", 400); return }
	copyFileOrDir(realPath(uid, src), realPath(uid, dst))
	writeJSON(w, map[string]string{"status": "ok"})
}

func copyFileOrDir(src, dst string) error {
	srcInfo, err := os.Stat(src)
	if err != nil { return err }
	if srcInfo.IsDir() {
		os.MkdirAll(dst, 0755)
		entries, _ := os.ReadDir(src)
		for _, e := range entries {
			copyFileOrDir(filepath.Join(src, e.Name()), filepath.Join(dst, e.Name()))
		}
		return nil
	}
	in, err := os.Open(src)
	if err != nil { return err }
	defer in.Close()
	os.MkdirAll(filepath.Dir(dst), 0755)
	out, err := os.Create(dst)
	if err != nil { return err }
	defer out.Close()
	_, err = io.Copy(out, in)
	return err
}

func handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	// Soft delete: move to trash
	src := realPath(uid, vpath)
	trashName := fmt.Sprintf("%d_%s", time.Now().UnixNano(), filepath.Base(vpath))
	dst := filepath.Join(trashDir(uid), trashName)
	os.Rename(src, dst)
	// Store original path for restore
	os.WriteFile(dst+".orig", []byte(vpath), 0644)
	logActivity(uid, vpath, filepath.Base(vpath), "deleted")
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleRestore(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	trashName := r.URL.Query().Get("id")
	src := filepath.Join(trashDir(uid), trashName)
	origData, _ := os.ReadFile(src + ".orig")
	origPath := string(origData)
	if origPath == "" { http.Error(w, "no original path", 400); return }
	dst := realPath(uid, origPath)
	os.MkdirAll(filepath.Dir(dst), 0755)
	os.Rename(src, dst)
	os.Remove(src + ".orig")
	logActivity(uid, origPath, filepath.Base(origPath), "restored")
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleDeletePermanent(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	trashName := r.URL.Query().Get("id")
	os.RemoveAll(filepath.Join(trashDir(uid), trashName))
	os.Remove(filepath.Join(trashDir(uid), trashName+".orig"))
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleFavorite(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	favs := loadFavorites(uid)
	favs[vpath] = !favs[vpath]
	saveFavorites(uid, favs)
	writeJSON(w, map[string]any{"status": "ok", "favorited": favs[vpath]})
}


func loadTags(uid string) map[string]Tag {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "tags.json"))
	var m map[string]Tag
	json.Unmarshal(data, &m)
	if m == nil { m = map[string]Tag{} }
	return m
}
func saveTags(uid string, m map[string]Tag) {
	data, _ := json.Marshal(m)
	os.MkdirAll(filepath.Join(*dataRoot, uid), 0755)
	os.WriteFile(filepath.Join(*dataRoot, uid, "tags.json"), data, 0644)
}
func loadFileTags(uid string) map[string][]string {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "file_tags.json"))
	var m map[string][]string
	json.Unmarshal(data, &m)
	if m == nil { m = map[string][]string{} }
	return m
}
func saveFileTags(uid string, m map[string][]string) {
	data, _ := json.Marshal(m)
	os.MkdirAll(filepath.Join(*dataRoot, uid), 0755)
	os.WriteFile(filepath.Join(*dataRoot, uid, "file_tags.json"), data, 0644)
}

func handleTags(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "GET":
		writeJSON(w, map[string]any{"tags": loadTags(uid)})
	case "POST":
		var t Tag
		json.NewDecoder(r.Body).Decode(&t)
		if t.Color == "" { t.Color = "#6366f1" }
		tags := loadTags(uid)
		t.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		tags[t.ID] = t
		saveTags(uid, tags)
		writeJSON(w, t)
	case "DELETE":
		id := r.URL.Query().Get("id")
		tags := loadTags(uid)
		delete(tags, id)
		saveTags(uid, tags)
		writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleTag(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	var req struct {
		Path string `json:"path"`
		Tag  string `json:"tag"`
		Add  *bool  `json:"add"`
	}
	json.NewDecoder(r.Body).Decode(&req)
	ft := loadFileTags(uid)
	list := ft[req.Path]
	if req.Add == nil || *req.Add {
		// Add tag
		for _, t := range list {
			if t == req.Tag { writeJSON(w, map[string]any{"status": "ok", "tags": list}); return }
		}
		ft[req.Path] = append(list, req.Tag)
	} else {
		// Remove tag
		var newList []string
		for _, t := range list {
			if t != req.Tag { newList = append(newList, t) }
		}
		ft[req.Path] = newList
	}
	saveFileTags(uid, ft)
	writeJSON(w, map[string]any{"status": "ok", "tags": ft[req.Path]})
}

// Update handleList to include tags and support tag filter
func handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	q := strings.ToLower(r.URL.Query().Get("q"))
	typeFilter := strings.ToLower(r.URL.Query().Get("type"))
	folderScope := r.URL.Query().Get("folder")
	if q == "" && typeFilter == "" { writeJSON(w, FileResponse{Files: []FileEntry{}, Path: "/"}); return }
	favs := loadFavorites(uid)
	var results []FileEntry
	roots := []string{userHome(uid), filesDir(uid)}
	if folderScope != "" { roots = []string{realPath(uid, folderScope)} }
	for _, root := range roots {
		filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
			if err != nil || p == root { return nil }
			matchesName := q == "" || strings.Contains(strings.ToLower(info.Name()), q)
			matchesType := true
			if typeFilter != "" {
				switch typeFilter {
				case "folder": matchesType = info.IsDir()
				case "image": matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "image/")
				case "document": matchesType = !info.IsDir() && (strings.HasSuffix(info.Name(), ".txt") || strings.HasSuffix(info.Name(), ".md") || strings.HasSuffix(info.Name(), ".pdf"))
				case "video": matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "video/")
				case "audio": matchesType = !info.IsDir() && strings.HasPrefix(mime.TypeByExtension(filepath.Ext(info.Name())), "audio/")
				}
			}
			if matchesName && matchesType {
				rel, _ := filepath.Rel(userHome(uid), p)
				vp := "/" + filepath.ToSlash(rel)
				if root == filesDir(uid) {
					rel2, _ := filepath.Rel(filesDir(uid), p)
					vp = "/files/" + filepath.ToSlash(rel2)
				}
				results = append(results, makeEntry(info, vp, favs, loadFileTags(uid)))
			}
			if len(results) >= 100 { return filepath.SkipAll }
			return nil
		})
	}
	if results == nil { results = []FileEntry{} }
	writeJSON(w, FileResponse{Files: results, Path: "/"})
}



func versionsDir(uid, vpath string) string {
	return filepath.Join(*dataRoot, uid, "versions", filepath.Clean(vpath))
}

func saveVersion(uid, vpath string) {
	src := realPath(uid, vpath)
	data, err := os.ReadFile(src)
	if err != nil { return }
	vd := versionsDir(uid, vpath)
	os.MkdirAll(vd, 0755)
	ts := fmt.Sprintf("%d", time.Now().UnixNano())
	os.WriteFile(filepath.Join(vd, ts), data, 0644)
}

func handleVersions(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	vd := versionsDir(uid, vpath)
	entries, _ := os.ReadDir(vd)
	type Version struct {
		ID   string `json:"id"`
		Size int64  `json:"size"`
		Time int64  `json:"time"`
	}
	var vers []Version
	for _, e := range entries {
		if e.IsDir() { continue }
		info, _ := e.Info()
		ts, _ := strconv.ParseInt(e.Name(), 10, 64)
		vers = append(vers, Version{ID: e.Name(), Size: info.Size(), Time: ts})
	}
	sort.Slice(vers, func(i, j int) bool { return vers[i].Time > vers[j].Time })
	if vers == nil { vers = []Version{} }
	writeJSON(w, map[string]any{"versions": vers})
}

func handleVersion(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	vid := r.URL.Query().Get("id")
	switch r.Method {
	case "GET":
		http.ServeFile(w, r, filepath.Join(versionsDir(uid, vpath), vid))
	case "POST":
		// Restore
		vd := versionsDir(uid, vpath)
		src := filepath.Join(vd, vid)
		dst := realPath(uid, vpath)
		// Save current as version before restoring
		saveVersion(uid, vpath)
		data, _ := os.ReadFile(src)
		os.MkdirAll(filepath.Dir(dst), 0755)
		os.WriteFile(dst, data, 0644)
		writeJSON(w, map[string]string{"status": "restored"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}


func handleUserShare(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case "POST":
		var req struct {
			Path       string `json:"path"`
			ToUser     string `json:"to_user"`
			Permission string `json:"permission"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		if req.Permission == "" { req.Permission = "read" }
		uid := userID(r)
		share := UserShare{ID: fmt.Sprintf("%d", time.Now().UnixNano()),
			Path: req.Path, FromUser: uid, ToUser: req.ToUser,
			Permission: req.Permission, Created: time.Now().Unix()}
		from := loadUserShares(uid); from = append(from, share); saveUserShares(uid, from)
		to := loadUserShares(req.ToUser); to = append(to, share); saveUserShares(req.ToUser, to)
		writeJSON(w, share)
	case "DELETE":
		uid := userID(r)
		id := r.URL.Query().Get("id")
		shares := loadUserShares(uid)
		for i, s := range shares {
			if s.ID == id { shares = append(shares[:i], shares[i+1:]...); saveUserShares(uid, shares); writeJSON(w, map[string]string{"status": "ok"}); return }
		}
		http.Error(w, "not found", 404)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleSharedWithMe(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	writeJSON(w, map[string]any{"shares": loadUserShares(uid)})
}


func handleDiff(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	v1 := r.URL.Query().Get("v1")
	v2 := r.URL.Query().Get("v2")
	vd := versionsDir(uid, vpath)
	d1, _ := os.ReadFile(filepath.Join(vd, v1))
	d2, _ := os.ReadFile(filepath.Join(vd, v2))
	lines1 := strings.Split(string(d1), "\n")
	lines2 := strings.Split(string(d2), "\n")
	var diff []string
	i, j := 0, 0
	for i < len(lines1) || j < len(lines2) {
		if i < len(lines1) && j < len(lines2) && lines1[i] == lines2[j] {
			diff = append(diff, "  "+lines1[i]); i++; j++
		} else if i < len(lines1) && (j >= len(lines2) || lines1[i] != lines2[j]) {
			diff = append(diff, "- "+lines1[i]); i++
		} else {
			diff = append(diff, "+ "+lines2[j]); j++
		}
	}
	writeJSON(w, map[string]any{"diff": strings.Join(diff, "\n")})
}

func handleStats(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	var total int64
	for _, root := range []string{userHome(uid), filesDir(uid)} {
		filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
			if err != nil { return nil }
			if !info.IsDir() { total += info.Size() }
			return nil
		})
	}
	writeJSON(w, map[string]any{"used": total, "total": int64(10 * 1024 * 1024 * 1024)})
}


type Activity struct {
	ID   string `json:"id"`
	Path string `json:"path"`
	Name string `json:"name"`
	Action string `json:"action"`
	Time int64  `json:"time"`
}

func logActivity(uid, path, name, action string) {
	d := filepath.Join(*dataRoot, uid, "activity.json")
	var acts []Activity
	data, _ := os.ReadFile(d)
	json.Unmarshal(data, &acts)
	acts = append(acts, Activity{
		ID: fmt.Sprintf("%d", time.Now().UnixNano()),
		Path: path, Name: name, Action: action, Time: time.Now().Unix(),
	})
	if len(acts) > 500 { acts = acts[len(acts)-500:] }
	b, _ := json.Marshal(acts)
	os.WriteFile(d, b, 0644)
}



type Share struct {
	Token    string `json:"token"`
	Path     string `json:"path"`
	Password string `json:"password,omitempty"`
	Expires  int64  `json:"expires,omitempty"`
	Drop     bool   `json:"drop"`
	Created  int64  `json:"created"`
}

func loadShares(uid string) map[string]Share {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "shares.json"))
	var m map[string]Share
	json.Unmarshal(data, &m)
	if m == nil { m = map[string]Share{} }
	return m
}
func saveShares(uid string, m map[string]Share) {
	data, _ := json.Marshal(m)
	os.MkdirAll(filepath.Join(*dataRoot, uid), 0755)
	os.WriteFile(filepath.Join(*dataRoot, uid, "shares.json"), data, 0644)
}


type Comment struct {
	ID        string `json:"id"`
	Path      string `json:"path"`
	Author    string `json:"author"`
	Body      string `json:"body"`
	ParentID  string `json:"parent_id,omitempty"`
	CreatedAt int64  `json:"created_at"`
}

func loadComments(uid, vpath string) []Comment {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "comments", filepath.Clean(vpath)+".json"))
	var c []Comment
	json.Unmarshal(data, &c)
	return c
}
func saveComments(uid, vpath string, c []Comment) {
	dir := filepath.Join(*dataRoot, uid, "comments")
	p := filepath.Join(dir, filepath.Clean(vpath)+".json")
	os.MkdirAll(filepath.Dir(p), 0755)
	data, _ := json.Marshal(c)
	os.WriteFile(p, data, 0644)
}



type ExternalStorage struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Type     string `json:"type"`
	Config   map[string]string `json:"config"`
	MountPath string `json:"mount_path"`
}

func handleExternal(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	cfgPath := filepath.Join(*dataRoot, uid, "external_storage.json")
	switch r.Method {
	case "GET":
		data, _ := os.ReadFile(cfgPath)
		var stores []ExternalStorage
		json.Unmarshal(data, &stores)
		if stores == nil { stores = []ExternalStorage{} }
		writeJSON(w, map[string]any{"stores": stores})
	case "POST":
		var s ExternalStorage
		json.NewDecoder(r.Body).Decode(&s)
		s.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		var stores []ExternalStorage
		data, _ := os.ReadFile(cfgPath)
		json.Unmarshal(data, &stores)
		stores = append(stores, s)
		b, _ := json.Marshal(stores)
		os.MkdirAll(filepath.Dir(cfgPath), 0755)
		os.WriteFile(cfgPath, b, 0644)
		writeJSON(w, s)
	case "DELETE":
		id := r.URL.Query().Get("id")
		var stores []ExternalStorage
		data, _ := os.ReadFile(cfgPath)
		json.Unmarshal(data, &stores)
		for i, s := range stores {
			if s.ID == id { stores = append(stores[:i], stores[i+1:]...); break }
		}
		b, _ := json.Marshal(stores)
		os.WriteFile(cfgPath, b, 0644)
		writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleWebDAV(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := "/" + strings.TrimPrefix(r.URL.Path, "/webdav/")
	rp := realPath(uid, vpath)
	switch r.Method {
	case "PROPFIND":
		// Basic PROPFIND response
		w.Header().Set("Content-Type", "application/xml; charset=utf-8")
		w.Header().Set("DAV", "1,2")
		w.WriteHeader(207)
		entries, _ := os.ReadDir(rp)
		xml := `<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:">`
		for _, e := range entries {
			info, _ := e.Info()
			href := "/webdav/" + strings.TrimPrefix(vpath, "/") + "/" + e.Name()
			if vpath == "/" { href = "/webdav/" + e.Name() }
			xml += `<d:response><d:href>` + href + `</d:href><d:propstat><d:prop>`
			if e.IsDir() { xml += `<d:resourcetype><d:collection/></d:resourcetype>` }
			xml += `<d:getcontentlength>` + fmt.Sprintf("%d", info.Size()) + `</d:getcontentlength>`
			xml += `<d:getlastmodified>` + info.ModTime().Format(time.RFC3339) + `</d:getlastmodified>`
			xml += `</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`
		}
		xml += `</d:multistatus>`
		w.Write([]byte(xml))
	case "MKCOL":
		os.MkdirAll(rp, 0755)
		w.WriteHeader(201)
	case "PUT":
		os.MkdirAll(filepath.Dir(rp), 0755)
		f, _ := os.Create(rp)
		if f != nil { defer f.Close(); io.Copy(f, r.Body) }
		w.WriteHeader(201)
	case "DELETE":
		os.RemoveAll(rp)
		w.WriteHeader(204)
	case "GET":
		info, err := os.Stat(rp)
		if err != nil { http.Error(w, "not found", 404); return }
		if info.IsDir() {
			entries, _ := os.ReadDir(rp)
			html := `<html><body><h1>` + vpath + `</h1><ul>`
			for _, e := range entries { html += `<li><a href="` + r.URL.Path + "/" + e.Name() + `">` + e.Name() + `</a></li>` }
			html += `</ul></body></html>`
			w.Header().Set("Content-Type", "text/html")
			w.Write([]byte(html))
			return
		}
		http.ServeFile(w, r, rp)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleComments(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	switch r.Method {
	case "GET":
		comments := loadComments(uid, vpath)
		if comments == nil { comments = []Comment{} }
		writeJSON(w, map[string]any{"comments": comments})
	case "POST":
		var c Comment
		json.NewDecoder(r.Body).Decode(&c)
		c.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		c.Path = vpath
		c.Author = uid
		c.CreatedAt = time.Now().Unix()
		comments := loadComments(uid, vpath)
		comments = append(comments, c)
		saveComments(uid, vpath, comments)
		writeJSON(w, c)
	case "DELETE":
		id := r.URL.Query().Get("id")
		comments := loadComments(uid, vpath)
		for i, c := range comments {
			if c.ID == id { comments = append(comments[:i], comments[i+1:]...); break }
		}
		saveComments(uid, vpath, comments)
		writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleShare(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case "POST":
		uid := userID(r)
		var req struct {
			Path     string `json:"path"`
			Password string `json:"password"`
			Expires  int64  `json:"expires"`
			Drop     bool   `json:"drop"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		shares := loadShares(uid)
		token := fmt.Sprintf("%x", time.Now().UnixNano())
		shares[token] = Share{
			Token: token, Path: req.Path, Password: req.Password,
			Drop: req.Drop, Expires: req.Expires, Created: time.Now().Unix(),
		}
		saveShares(uid, shares)
		writeJSON(w, shares[token])
	case "GET":
		// Access shared file via token (no auth needed)
		token := r.URL.Query().Get("token")
		pw := r.URL.Query().Get("pw")
		// Search all users for this share
		entries, _ := os.ReadDir(*dataRoot)
		for _, e := range entries {
			if !e.IsDir() { continue }
			shares := loadShares(e.Name())
			if s, ok := shares[token]; ok {
				if s.Drop {
				w.Header().Set("Content-Type", "text/html")
				w.Write([]byte(`<html><body style="font-family:sans-serif;max-width:400px;margin:50px auto;text-align:center"><h2>File Drop</h2><p>Upload files to ` + filepath.Base(s.Path) + `</p><form method="POST" enctype="multipart/form-data"><input type="file" name="file" multiple style="margin:10px 0"><br><button style="padding:8px 20px;border-radius:20px;background:#6366f1;color:white;border:none;cursor:pointer">Upload</button></form></body></html>`))
				return
			}
			// Handle file upload for drop mode
			if s.Drop && r.Method == "POST" {
				r.ParseMultipartForm(128 << 20)
				if mr := r.MultipartForm; mr != nil {
					for _, fhs := range mr.File {
						for _, fh := range fhs {
							saveUploadedFile(e.Name(), s.Path, fh)
						}
					}
				}
				w.Write([]byte(`<html><body style="font-family:sans-serif;text-align:center;margin-top:50px"><h2>Files uploaded!</h2><a href="?token=` + token + `">Upload more</a></body></html>`))
				return
			}
			if s.Password != "" && s.Password != pw {
					w.Header().Set("Content-Type", "text/html")
					w.Write([]byte(`<html><body><form method="GET"><input name="token" type="hidden" value="` + token + `"><input name="pw" placeholder="Password" type="password"><button>Unlock</button></form></body></html>`))
					return
				}
				if s.Expires > 0 && s.Expires < time.Now().Unix() {
					http.Error(w, "share expired", 410)
					return
				}
				http.ServeFile(w, r, realPath(e.Name(), s.Path))
				return
			}
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		uid := userID(r)
		token := r.URL.Query().Get("token")
		shares := loadShares(uid)
		delete(shares, token)
		saveShares(uid, shares)
		writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}


func handleShared(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	shares := loadShares(uid)
	writeJSON(w, map[string]any{"shares": shares})
}


func loadUserShares(uid string) []UserShare {
	data, _ := os.ReadFile(filepath.Join(*dataRoot, uid, "user_shares.json"))
	var s []UserShare
	json.Unmarshal(data, &s)
	if s == nil { s = []UserShare{} }
	return s
}
func saveUserShares(uid string, s []UserShare) {
	data, _ := json.Marshal(s)
	os.MkdirAll(filepath.Join(*dataRoot, uid), 0755)
	os.WriteFile(filepath.Join(*dataRoot, uid, "user_shares.json"), data, 0644)
}

func handleLock(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	lockFile := filepath.Join(*dataRoot, uid, "locks.json")
	var locks map[string]string
	data, _ := os.ReadFile(lockFile)
	json.Unmarshal(data, &locks)
	if locks == nil { locks = map[string]string{} }
	switch r.Method {
	case "GET":
		owner, locked := locks[vpath]
		writeJSON(w, map[string]any{"locked": locked, "owner": owner})
	case "POST":
		locks[vpath] = uid
		b, _ := json.Marshal(locks)
		os.WriteFile(lockFile, b, 0644)
		writeJSON(w, map[string]any{"status": "locked"})
	case "DELETE":
		delete(locks, vpath)
		b, _ := json.Marshal(locks)
		os.WriteFile(lockFile, b, 0644)
		writeJSON(w, map[string]any{"status": "unlocked"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleActivity(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	d := filepath.Join(*dataRoot, uid, "activity.json")
	var acts []Activity
	data, _ := os.ReadFile(d)
	json.Unmarshal(data, &acts)
	if acts == nil { acts = []Activity{} }
	sort.Slice(acts, func(i, j int) bool { return acts[i].Time > acts[j].Time })
	limit := 50
	if len(acts) > limit { acts = acts[:limit] }
	writeJSON(w, map[string]any{"activity": acts})
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}


func handleThumbnail(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "/Shared" {
		shares := loadUserShares(uid)
		var files []FileEntry
		for _, s := range shares {
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: s.Path, IsDir: false, UpdatedAt: s.Created})
		}
		if files == nil { files = []FileEntry{} }
		writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	rp := realPath(uid, vpath)
	src, err := os.Open(rp)
	if err != nil { http.Error(w, "not found", 404); return }
	defer src.Close()
	img, _, err := image.Decode(src)
	if err != nil { http.Error(w, "not an image", 415); return }
	b := img.Bounds()
	maxW := 200
	ratio := float64(maxW) / float64(b.Dx())
	newH := int(float64(b.Dy()) * ratio)
	if b.Dx() <= maxW { newH = b.Dy(); maxW = b.Dx() }
	scaled := image.NewRGBA(image.Rect(0, 0, maxW, newH))
	for y := 0; y < newH; y++ {
		for x := 0; x < maxW; x++ {
			sx := int(float64(x) / ratio)
			if b.Dx() <= maxW { sx = x }
			sy := int(float64(y) / ratio)
			if b.Dx() <= maxW { sy = y }
			scaled.Set(x, y, img.At(sx, sy))
		}
	}
	w.Header().Set("Content-Type", "image/jpeg")
	jpeg.Encode(w, scaled, &jpeg.Options{Quality: 70})
}

func handlePreview(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "/Shared" {
		shares := loadUserShares(uid)
		var files []FileEntry
		for _, s := range shares {
			files = append(files, FileEntry{Name: filepath.Base(s.Path) + " (from " + s.FromUser + ")", Path: s.Path, IsDir: false, UpdatedAt: s.Created})
		}
		if files == nil { files = []FileEntry{} }
		writeJSON(w, FileResponse{Files: files, Path: "/Shared"})
		return
	}

	rp := realPath(uid, vpath)
	data, err := os.ReadFile(rp)
	if err != nil { http.Error(w, "not found", 404); return }
	mtype := mime.TypeByExtension(filepath.Ext(vpath))
	content := string(data)
	if len(content) > 512*1024 { content = content[:512*1024] }
	writeJSON(w, map[string]string{"type": mtype, "content": content, "name": filepath.Base(vpath)})
}

func addTaggedFiles(uid, root, prefix string, ft map[string][]string, tagID string, favs map[string]bool, files *[]FileEntry) {
	filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
		if err != nil || p == root { return nil }
		rel, _ := filepath.Rel(root, p)
		vp := prefix + "/" + filepath.ToSlash(rel)
		for _, t := range ft[vp] {
			if t == tagID {
				*files = append(*files, makeEntry(info, vp, favs, ft))
				if info.IsDir() { return filepath.SkipDir }
				return nil
			}
		}
		return nil
	})
}
