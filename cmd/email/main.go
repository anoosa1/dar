// Email daemon — local email storage with REST API.
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
	"time"
)

var (
	port     = flag.Int("port", 8088, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/folders", handleFolders)
	mux.HandleFunc("/messages", handleMessages)
	mux.HandleFunc("/message", handleMessage)
	mux.HandleFunc("/health", handleHealth)
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[email] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Email struct {
	ID       string   `json:"id"`
	Folder   string   `json:"folder"`
	From     string   `json:"from"`
	To       string   `json:"to"`
	Subject  string   `json:"subject"`
	Body     string   `json:"body"`
	Date     int64    `json:"date"`
	Read     bool     `json:"read"`
	Starred  bool     `json:"starred"`
}

type Folder struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

func emailDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "email")
	os.MkdirAll(d, 0755)
	return d
}

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func loadMessages(uid string) []Email {
	data, _ := os.ReadFile(filepath.Join(emailDir(uid), "messages.json"))
	var m []Email
	json.Unmarshal(data, &m)
	if m == nil { m = []Email{} }
	return m
}

func saveMessages(uid string, m []Email) {
	data, _ := json.Marshal(m)
	os.WriteFile(filepath.Join(emailDir(uid), "messages.json"), data, 0644)
}

func loadFolders(uid string) []Folder {
	data, _ := os.ReadFile(filepath.Join(emailDir(uid), "folders.json"))
	var f []Folder
	json.Unmarshal(data, &f)
	if f == nil { f = []Folder{
		{ID: "inbox", Name: "Inbox"}, {ID: "sent", Name: "Sent"},
		{ID: "drafts", Name: "Drafts"}, {ID: "trash", Name: "Trash"},
	}}
	return f
}

func handleFolders(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	writeJSON(w, map[string]any{"folders": loadFolders(uid)})
}

func handleMessages(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	folder := r.URL.Query().Get("folder")
	if folder == "" { folder = "inbox" }
	messages := loadMessages(uid)
	var result []Email
	for _, m := range messages {
		if m.Folder == folder {
			result = append(result, m)
		}
	}
	sort.Slice(result, func(i, j int) bool { return result[i].Date > result[j].Date })
	writeJSON(w, map[string]any{"messages": result})
}

func handleMessage(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "GET":
		id := r.URL.Query().Get("id")
		messages := loadMessages(uid)
		for i, m := range messages {
			if m.ID == id {
				messages[i].Read = true
				saveMessages(uid, messages)
				writeJSON(w, m)
				return
			}
		}
		http.Error(w, "not found", 404)
	case "POST":
		// Compose/send
		var e Email
		json.NewDecoder(r.Body).Decode(&e)
		if e.Folder == "" { e.Folder = "sent" }
		if e.From == "" { e.From = userID(r) + "@dar.local" }
		e.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		e.Date = time.Now().Unix()
		messages := loadMessages(uid)
		messages = append(messages, e)
		saveMessages(uid, messages)
		writeJSON(w, e)
	case "PUT":
		// Move to folder
		id := r.URL.Query().Get("id")
		folder := r.URL.Query().Get("folder")
		messages := loadMessages(uid)
		for i := range messages {
			if messages[i].ID == id {
				messages[i].Folder = folder
				saveMessages(uid, messages)
				writeJSON(w, messages[i])
				return
			}
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		id := r.URL.Query().Get("id")
		messages := loadMessages(uid)
		for i, m := range messages {
			if m.ID == id {
				messages = append(messages[:i], messages[i+1:]...)
				saveMessages(uid, messages)
				writeJSON(w, map[string]string{"status": "ok"})
				return
			}
		}
		http.Error(w, "not found", 404)
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
