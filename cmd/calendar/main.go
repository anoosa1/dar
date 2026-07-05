// Calendar daemon — iCal storage with REST API.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

var (
	port     = flag.Int("port", 8084, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)

	mux := http.NewServeMux()
	mux.HandleFunc("/list", handleList)
	mux.HandleFunc("/get", handleGet)
	mux.HandleFunc("/create", handleCreate)
	mux.HandleFunc("/update", handleUpdate)
	mux.HandleFunc("/delete", handleDelete)
	mux.HandleFunc("/health", handleHealth)

	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[calendar] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Event struct {
	ID          string `json:"id"`
	Title       string `json:"title"`
	Description string `json:"description"`
	Start       int64  `json:"start"`
	End         int64  `json:"end"`
	AllDay      bool   `json:"all_day"`
	Color       string `json:"color"`
}

func eventsDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "calendar")
	os.MkdirAll(d, 0755)
	return d
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

func parseICS(path string) (Event, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Event{}, err
	}
	e := Event{ID: strings.TrimSuffix(filepath.Base(path), ".ics")}
	lines := strings.Split(string(data), "\n")
	for _, line := range lines {
		switch {
		case strings.HasPrefix(line, "SUMMARY:"):
			e.Title = strings.TrimPrefix(line, "SUMMARY:")
		case strings.HasPrefix(line, "DESCRIPTION:"):
			e.Description = strings.TrimPrefix(line, "DESCRIPTION:")
		case strings.HasPrefix(line, "DTSTART:"):
			e.Start = parseICalTime(strings.TrimPrefix(line, "DTSTART:"))
		case strings.HasPrefix(line, "DTEND:"):
			e.End = parseICalTime(strings.TrimPrefix(line, "DTEND:"))
		case strings.HasPrefix(line, "COLOR:"):
			e.Color = strings.TrimPrefix(line, "COLOR:")
		}
	}
	return e, nil
}

func parseICalTime(s string) int64 {
	s = strings.TrimSpace(s)
	format := "20060102T150405Z"
	if len(s) == 8 {
		format = "20060102"
	}
	t, err := time.Parse(format, s)
	if err != nil {
		return 0
	}
	return t.Unix()
}

func toICS(e Event) string {
	startFmt := "20060102T150405Z"
	if e.AllDay {
		startFmt = "20060102"
	}
	var b strings.Builder
	b.WriteString("BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\n")
	b.WriteString(fmt.Sprintf("DTSTART:%s\n", time.Unix(e.Start, 0).UTC().Format(startFmt)))
	b.WriteString(fmt.Sprintf("DTEND:%s\n", time.Unix(e.End, 0).UTC().Format(startFmt)))
	if e.Title != "" {
		b.WriteString("SUMMARY:" + e.Title + "\n")
	}
	if e.Description != "" {
		b.WriteString("DESCRIPTION:" + e.Description + "\n")
	}
	b.WriteString("END:VEVENT\nEND:VCALENDAR\n")
	return b.String()
}

func handleList(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	dir := eventsDir(uid)
	entries, _ := os.ReadDir(dir)
	var events []Event
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".ics") {
			continue
		}
		e, err := parseICS(filepath.Join(dir, entry.Name()))
		if err != nil {
			continue
		}
		events = append(events, e)
	}

	// Filter by date range if provided
	startStr := r.URL.Query().Get("start")
	endStr := r.URL.Query().Get("end")
	if startStr != "" && endStr != "" {
		var start, end int64
		fmt.Sscanf(startStr, "%d", &start)
		fmt.Sscanf(endStr, "%d", &end)
		filtered := []Event{}
		for _, e := range events {
			if e.Start < end && e.End > start {
				filtered = append(filtered, e)
			}
		}
		events = filtered
	}

	if events == nil {
		events = []Event{}
	}
	writeJSON(w, map[string]any{"events": events})
}

func handleGet(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	id := r.URL.Query().Get("id")
	e, err := parseICS(filepath.Join(eventsDir(uid), id+".ics"))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	writeJSON(w, e)
}

func handleCreate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var e Event
	json.NewDecoder(r.Body).Decode(&e)
	e.ID = fmt.Sprintf("%d", time.Now().UnixNano())
	os.WriteFile(filepath.Join(eventsDir(userID(r)), e.ID+".ics"), []byte(toICS(e)), 0644)
	writeJSON(w, e)
}

func handleUpdate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var e Event
	json.NewDecoder(r.Body).Decode(&e)
	if e.ID == "" {
		http.Error(w, "missing id", 400)
		return
	}
	os.WriteFile(filepath.Join(eventsDir(userID(r)), e.ID+".ics"), []byte(toICS(e)), 0644)
	writeJSON(w, e)
}

func handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	os.Remove(filepath.Join(eventsDir(uid), r.URL.Query().Get("id")+".ics"))
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
