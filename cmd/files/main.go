// Files daemon — file storage with REST API. Simpler: disk-based, no SQLite.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"mime"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

var (
	port      = flag.Int("port", 8082, "HTTP port")
	dataRoot  = flag.String("data-root", "data", "Root directory for user data")
	soosaPort = flag.Int("soosa-port", 0, "Soosa daemon port for tool registration")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)

	mux := http.NewServeMux()
	mux.HandleFunc("/list", handleList)
	mux.HandleFunc("/upload", handleUpload)
	mux.HandleFunc("/download", handleDownload)
	mux.HandleFunc("/mkdir", handleMkdir)
	mux.HandleFunc("/delete", handleDelete)
	mux.HandleFunc("/move", handleMove)
	mux.HandleFunc("/search", handleSearch)
	mux.HandleFunc("/health", handleHealth)

	addr := fmt.Sprintf(":%d", *port)
	if *soosaPort > 0 {
		go registerSoosaTools()
	}
	log.Printf("[files] listening on :%d (data: %s)", *port, *dataRoot)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type FileEntry struct {
	Name      string `json:"name"`
	Path      string `json:"path"`
	Size      int64  `json:"size"`
	MimeType  string `json:"mime_type"`
	IsDir     bool   `json:"is_dir"`
	UpdatedAt int64  `json:"updated_at"`
}

type FileResponse struct {
	Files []FileEntry `json:"files"`
	Path  string      `json:"path"`
}

func userDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "files")
	os.MkdirAll(d, 0755)
	return d
}

func realPath(uid, vpath string) string {
	return filepath.Join(userDir(uid), filepath.Clean("/"+vpath))
}

func userID(r *http.Request) string {
	uid := r.Header.Get("X-User-Id")
	if uid == "" {
		uid = r.URL.Query().Get("user_id")
	}
	if uid == "" {
		uid = "default"
	}
	return uid
}

func handleList(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	rp := realPath(uid, vpath)

	entries, _ := os.ReadDir(rp)
	var files []FileEntry
	for _, e := range entries {
		name := e.Name()
		childPath := vpath + "/" + name
		if vpath == "/" {
			childPath = "/" + name
		}
		info, _ := e.Info()
		mtype := ""
		if !e.IsDir() {
			mtype = mime.TypeByExtension(filepath.Ext(name))
		}
		files = append(files, FileEntry{
			Name:      name,
			Path:      childPath,
			Size:      info.Size(),
			MimeType:  mtype,
			IsDir:     e.IsDir(),
			UpdatedAt: info.ModTime().Unix(),
		})
	}
	if files == nil {
		files = []FileEntry{}
	}
	writeJSON(w, FileResponse{Files: files, Path: vpath})
}

func handleUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	if vpath == "" {
		vpath = "/"
	}
	r.ParseMultipartForm(64 << 20)
	file, header, err := r.FormFile("file")
	if err != nil {
		http.Error(w, "no file: "+err.Error(), 400)
		return
	}
	defer file.Close()

	name := header.Filename
	fullPath := vpath + "/" + name
	if vpath == "/" {
		fullPath = "/" + name
	}
	rp := realPath(uid, fullPath)
	os.MkdirAll(filepath.Dir(rp), 0755)

	dst, err := os.Create(rp)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	defer dst.Close()
	io.Copy(dst, file)
	writeJSON(w, map[string]any{"status": "ok", "name": name})
}

func handleDownload(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	http.ServeFile(w, r, realPath(uid, vpath))
}

func handleMkdir(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	name := r.URL.Query().Get("name")
	if name == "" {
		http.Error(w, "missing name", 400)
		return
	}
	fullPath := vpath + "/" + name
	if vpath == "/" {
		fullPath = "/" + name
	}
	if err := os.MkdirAll(realPath(uid, fullPath), 0755); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	vpath := r.URL.Query().Get("path")
	os.RemoveAll(realPath(uid, vpath))
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleMove(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	src := r.URL.Query().Get("src")
	dst := r.URL.Query().Get("dst")
	if src == "" || dst == "" {
		http.Error(w, "missing src or dst", 400)
		return
	}
	if err := os.Rename(realPath(uid, src), realPath(uid, dst)); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	query := r.URL.Query().Get("q")
	if query == "" {
		writeJSON(w, FileResponse{Files: []FileEntry{}, Path: "/"})
		return
	}
	root := userDir(uid)
	var results []FileEntry
	filepath.Walk(root, func(walkPath string, info os.FileInfo, err error) error {
		if err != nil || walkPath == root {
			return nil
		}
		if strings.Contains(strings.ToLower(info.Name()), strings.ToLower(query)) {
			rel, _ := filepath.Rel(root, walkPath)
			vpath := "/" + filepath.ToSlash(rel)
			mtype := ""
			if !info.IsDir() {
				mtype = mime.TypeByExtension(filepath.Ext(info.Name()))
			}
			results = append(results, FileEntry{
				Name:      info.Name(),
				Path:      vpath,
				Size:      info.Size(),
				MimeType:  mtype,
				IsDir:     info.IsDir(),
				UpdatedAt: info.ModTime().Unix(),
			})
			if len(results) >= 100 {
				return filepath.SkipAll
			}
		}
		return nil
	})
	if results == nil {
		results = []FileEntry{}
	}
	writeJSON(w, FileResponse{Files: results, Path: "/"})
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

func registerSoosaTools() {
	soosaURL := fmt.Sprintf("http://localhost:%d/tools", *soosaPort)
	filesURL := fmt.Sprintf("http://localhost:%d", *port)
	tools := []map[string]string{
		{"name": "search_files", "description": "Search for files by name", "endpoint": filesURL + "/search"},
		{"name": "list_files", "description": "List files in a directory", "endpoint": filesURL + "/list"},
		{"name": "create_folder", "description": "Create a new folder", "endpoint": filesURL + "/mkdir"},
	}
	for _, t := range tools {
		body, _ := json.Marshal(t)
		resp, err := http.Post(soosaURL, "application/json", strings.NewReader(string(body)))
		if err != nil {
			log.Printf("[files] failed to register tool %s: %v", t["name"], err)
			continue
		}
		resp.Body.Close()
		log.Printf("[files] registered tool: %s", t["name"])
	}
}
