// Notes daemon — Markdown notes with REST API.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

var (
	port     = flag.Int("port", 8086, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/notes", handleNotes)
	mux.HandleFunc("/note", handleNote)
	mux.HandleFunc("/folders", handleFolders)
	mux.HandleFunc("/health", handleHealth)
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[notes] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Note struct {
	ID        string `json:"id"`
	FolderID  string `json:"folder_id"`
	Title     string `json:"title"`
	Content   string `json:"content"`
	Favorited bool   `json:"favorited"`
	Trashed   bool   `json:"trashed"`
	UpdatedAt int64  `json:"updated_at"`
}

type Folder struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

func notesDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "notes")
	os.MkdirAll(d, 0755)
	return d
}

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func loadNotes(uid string) []Note {
	data, _ := os.ReadFile(filepath.Join(notesDir(uid), "notes.json"))
	var notes []Note
	json.Unmarshal(data, &notes)
	if notes == nil { notes = []Note{} }
	return notes
}

func saveNotes(uid string, notes []Note) {
	data, _ := json.Marshal(notes)
	os.WriteFile(filepath.Join(notesDir(uid), "notes.json"), data, 0644)
}

func loadFolders(uid string) []Folder {
	data, _ := os.ReadFile(filepath.Join(notesDir(uid), "folders.json"))
	var f []Folder
	json.Unmarshal(data, &f)
	if f == nil { f = []Folder{{ID: "default", Name: "Notes"}} }
	return f
}

func saveFolders(uid string, f []Folder) {
	data, _ := json.Marshal(f)
	os.WriteFile(filepath.Join(notesDir(uid), "folders.json"), data, 0644)
}

func handleNotes(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	folderID := r.URL.Query().Get("folder_id")
	q := r.URL.Query().Get("q")
	notes := loadNotes(uid)
	var result []Note
	for _, n := range notes {
		if n.Trashed { continue }
		if folderID != "" && n.FolderID != folderID { continue }
		if q != "" && !strings.Contains(strings.ToLower(n.Title+" "+n.Content), strings.ToLower(q)) { continue }
		result = append(result, n)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].UpdatedAt > result[j].UpdatedAt })
	writeJSON(w, map[string]any{"notes": result})
}

func handleNote(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "POST":
		var n Note
		json.NewDecoder(r.Body).Decode(&n)
		if n.FolderID == "" { n.FolderID = "default" }
		n.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		if n.Title == "" { n.Title = "Untitled" }
		n.UpdatedAt = time.Now().Unix()
		notes := loadNotes(uid)
		notes = append(notes, n)
		saveNotes(uid, notes)
		writeJSON(w, n)
	case "PUT":
		var n Note
		json.NewDecoder(r.Body).Decode(&n)
		notes := loadNotes(uid)
		for i, e := range notes {
			if e.ID == n.ID {
				n.UpdatedAt = time.Now().Unix()
				if n.Title == "" { n.Title = e.Title }
				if n.Content == "" { n.Content = e.Content }
				notes[i] = n
				saveNotes(uid, notes)
				writeJSON(w, n)
				return
			}
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		id := r.URL.Query().Get("id")
		notes := loadNotes(uid)
		for i, n := range notes {
			if n.ID == id {
				notes = append(notes[:i], notes[i+1:]...)
				saveNotes(uid, notes)
				writeJSON(w, map[string]string{"status": "ok"})
				return
			}
		}
		http.Error(w, "not found", 404)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleFolders(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "GET":
		writeJSON(w, map[string]any{"folders": loadFolders(uid)})
	case "POST":
		var f Folder
		json.NewDecoder(r.Body).Decode(&f)
		f.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		folders := loadFolders(uid)
		folders = append(folders, f)
		saveFolders(uid, folders)
		writeJSON(w, f)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
