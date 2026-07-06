// Files daemon — disk-based storage with trash, favorites, text files, copy.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"image"
	"image/jpeg"
	"io"
	"log"
	"mime"
	"mime/multipart"
	"net/http"
	"os"
	"path/filepath"
	"sort"
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
	mux.HandleFunc("/thumb", handleThumbnail)
	mux.HandleFunc("/preview", handlePreview)
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
	Favorited bool   `json:"favorited"`
	TrashPath string `json:"trash_path,omitempty"`
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
					files = append(files, makeEntry(info, vp, favs))
					return filepath.SkipDir
				}
				if favs[vp] {
					files = append(files, makeEntry(info, vp, favs))
				}
				return nil
			})
		}
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
		files = append(files, makeEntry(info, childPath, favs))
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir { return files[i].IsDir }
		return strings.ToLower(files[i].Name) < strings.ToLower(files[j].Name)
	})
	if files == nil { files = []FileEntry{} }
	writeJSON(w, FileResponse{Files: files, Path: vpath})
}

func makeEntry(info os.FileInfo, path string, favs map[string]bool) FileEntry {
	mtype := ""
	if !info.IsDir() { mtype = mime.TypeByExtension(filepath.Ext(info.Name())) }
	return FileEntry{
		Name: info.Name(), Path: path, Size: info.Size(), MimeType: mtype,
		IsDir: info.IsDir(), UpdatedAt: info.ModTime().Unix(), Favorited: favs[path],
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
	dst, err := os.Create(rp)
	if err != nil { return }
	defer dst.Close()
	io.Copy(dst, file)
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
	os.WriteFile(rp, []byte(""), 0644)
	writeJSON(w, map[string]any{"status": "ok", "path": fullPath, "name": name})
}

func handleRename(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" { http.Error(w, "missing src or dst", 400); return }
	os.Rename(realPath(uid, src), realPath(uid, dst))
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
				results = append(results, makeEntry(info, vp, favs))
			}
			if len(results) >= 100 { return filepath.SkipAll }
			return nil
		})
	}
	if results == nil { results = []FileEntry{} }
	writeJSON(w, FileResponse{Files: results, Path: "/"})
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
