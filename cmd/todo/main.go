// Todo daemon — task storage with REST API.
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
	port     = flag.Int("port", 8085, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)

	mux := http.NewServeMux()
	mux.HandleFunc("/lists", handleLists)
	mux.HandleFunc("/tasks", handleTasks)
	mux.HandleFunc("/task", handleTask)
	mux.HandleFunc("/health", handleHealth)

	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[todo] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type TaskList struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type Task struct {
	ID        string `json:"id"`
	ListID    string `json:"list_id"`
	Title     string `json:"title"`
	Notes     string `json:"notes"`
	Completed bool   `json:"completed"`
	DueDate   string `json:"due_date"`
	Priority  int    `json:"priority"`
	CreatedAt int64  `json:"created_at"`
}

func todoDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "todo")
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

func loadTasks(uid string) []Task {
	data, err := os.ReadFile(filepath.Join(todoDir(uid), "tasks.json"))
	if err != nil {
		return []Task{}
	}
	var tasks []Task
	json.Unmarshal(data, &tasks)
	if tasks == nil {
		tasks = []Task{}
	}
	return tasks
}

func saveTasks(uid string, tasks []Task) error {
	data, _ := json.Marshal(tasks)
	return os.WriteFile(filepath.Join(todoDir(uid), "tasks.json"), data, 0644)
}

func loadLists(uid string) []TaskList {
	data, err := os.ReadFile(filepath.Join(todoDir(uid), "lists.json"))
	if err != nil {
		return []TaskList{{ID: "default", Name: "Tasks"}}
	}
	var lists []TaskList
	json.Unmarshal(data, &lists)
	if len(lists) == 0 {
		lists = []TaskList{{ID: "default", Name: "Tasks"}}
	}
	return lists
}

func saveLists(uid string, lists []TaskList) error {
	data, _ := json.Marshal(lists)
	return os.WriteFile(filepath.Join(todoDir(uid), "lists.json"), data, 0644)
}

func handleLists(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "GET":
		writeJSON(w, map[string]any{"lists": loadLists(uid)})
	case "POST":
		var l TaskList
		json.NewDecoder(r.Body).Decode(&l)
		l.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		lists := loadLists(uid)
		lists = append(lists, l)
		saveLists(uid, lists)
		writeJSON(w, l)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleTasks(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	listID := r.URL.Query().Get("list_id")
	if listID == "" {
		listID = "default"
	}
	filter := r.URL.Query().Get("filter") // "completed", "active", or empty for all
	tasks := loadTasks(uid)
	var result []Task
	for _, t := range tasks {
		if t.ListID != listID {
			continue
		}
		if filter == "completed" && !t.Completed {
			continue
		}
		if filter == "active" && t.Completed {
			continue
		}
		result = append(result, t)
	}
	sort.Slice(result, func(i, j int) bool {
		if result[i].Completed != result[j].Completed {
			return !result[i].Completed
		}
		return result[i].CreatedAt > result[j].CreatedAt
	})
	writeJSON(w, map[string]any{"tasks": result})
}

func handleTask(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "POST":
		var t Task
		json.NewDecoder(r.Body).Decode(&t)
		if t.ListID == "" {
			t.ListID = "default"
		}
		t.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		t.CreatedAt = time.Now().Unix()
		tasks := loadTasks(uid)
		tasks = append(tasks, t)
		saveTasks(uid, tasks)
		writeJSON(w, t)
	case "PUT":
		var t Task
		json.NewDecoder(r.Body).Decode(&t)
		tasks := loadTasks(uid)
		for i, existing := range tasks {
			if existing.ID == t.ID {
				tasks[i] = t
				saveTasks(uid, tasks)
				writeJSON(w, t)
				return
			}
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		id := r.URL.Query().Get("id")
		tasks := loadTasks(uid)
		for i, t := range tasks {
			if t.ID == id {
				tasks = append(tasks[:i], tasks[i+1:]...)
				saveTasks(uid, tasks)
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
