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
	mux.HandleFunc("/stats", handleStats)
	mux.HandleFunc("/activity", handleActivity)
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
	if q == "" { writeJSON(w, FileResponse{Files: []FileEntry{}, Path: "/"}); return }
	favs := loadFavorites(uid)
	var results []FileEntry
	for _, root := range []string{userHome(uid), filesDir(uid)} {
		filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
			if err != nil || p == root { return nil }
			if strings.Contains(strings.ToLower(info.Name()), q) {
				rel, _ := filepath.Rel(root, p)
				vp := "/" + filepath.ToSlash(rel)
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
