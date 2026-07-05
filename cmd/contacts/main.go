// Contacts daemon — vCard storage with REST API.
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
	port     = flag.Int("port", 8083, "HTTP port")
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
	mux.HandleFunc("/search", handleSearch)
	mux.HandleFunc("/health", handleHealth)

	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[contacts] listening on :%d (data: %s)", *port, *dataRoot)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Contact struct {
	ID        string `json:"id"`
	FirstName string `json:"first_name"`
	LastName  string `json:"last_name"`
	Email     string `json:"email"`
	Phone     string `json:"phone"`
	Org       string `json:"org"`
	Notes     string `json:"notes"`
	Color     string `json:"color"`
	UpdatedAt int64  `json:"updated_at"`
}

func contactsDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "contacts")
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

// vCard color palette
var colors = []string{"#6750a4", "#b3261e", "#006a6a", "#984061", "#196584", "#536d00", "#8b5000", "#006874"}

func contactColor(idx int) string { return colors[idx%len(colors)] }

func parseVCard(path string) (Contact, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Contact{}, err
	}
	c := Contact{
		ID: filepath.Base(path),
		UpdatedAt: func() int64 {
			s, _ := os.Stat(path)
			if s != nil {
				return s.ModTime().Unix()
			}
			return 0
		}(),
	}
	c.ID = strings.TrimSuffix(c.ID, ".vcf")
	lines := strings.Split(string(data), "\n")
	for _, line := range lines {
		if strings.HasPrefix(line, "FN:") {
			c.FirstName = strings.TrimPrefix(line, "FN:")
		} else if strings.HasPrefix(line, "N:") {
			parts := strings.SplitN(strings.TrimPrefix(line, "N:"), ";", 5)
			if len(parts) > 0 {
				c.LastName = parts[0]
			}
			if len(parts) > 1 {
				c.FirstName = parts[1]
			}
		} else if strings.HasPrefix(line, "EMAIL:") {
			c.Email = strings.TrimPrefix(line, "EMAIL:")
		} else if strings.HasPrefix(line, "TEL:") {
			c.Phone = strings.TrimPrefix(line, "TEL:")
		} else if strings.HasPrefix(line, "ORG:") {
			c.Org = strings.TrimPrefix(line, "ORG:")
		} else if strings.HasPrefix(line, "NOTE:") {
			c.Notes = strings.TrimPrefix(line, "NOTE:")
		}
	}
	if c.FirstName == "" && c.LastName == "" {
		c.FirstName = c.Email
	}
	return c, nil
}

func toVCard(c Contact) string {
	var b strings.Builder
	b.WriteString("BEGIN:VCARD\nVERSION:3.0\n")
	b.WriteString(fmt.Sprintf("FN:%s %s\n", c.FirstName, c.LastName))
	b.WriteString(fmt.Sprintf("N:%s;%s;;;\n", c.LastName, c.FirstName))
	if c.Email != "" {
		b.WriteString("EMAIL:" + c.Email + "\n")
	}
	if c.Phone != "" {
		b.WriteString("TEL:" + c.Phone + "\n")
	}
	if c.Org != "" {
		b.WriteString("ORG:" + c.Org + "\n")
	}
	if c.Notes != "" {
		b.WriteString("NOTE:" + c.Notes + "\n")
	}
	b.WriteString(fmt.Sprintf("REV:%s\n", time.Now().Format("20060102T150405Z")))
	b.WriteString("END:VCARD\n")
	return b.String()
}

func handleList(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	dir := contactsDir(uid)
	entries, _ := os.ReadDir(dir)
	type contactRow struct {
		Contact
		Initial string `json:"initial"`
	}
	var contacts []contactRow
	for i, e := range entries {
		if !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		c, err := parseVCard(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		c.Color = contactColor(i)
		initial := ""
		if c.FirstName != "" {
			initial = strings.ToUpper(c.FirstName[:1])
		} else if c.LastName != "" {
			initial = strings.ToUpper(c.LastName[:1])
		}
		contacts = append(contacts, contactRow{Contact: c, Initial: initial})
	}
	sort.Slice(contacts, func(i, j int) bool {
		a := contacts[i].FirstName
		if a == "" {
			a = contacts[i].LastName
		}
		b := contacts[j].FirstName
		if b == "" {
			b = contacts[j].LastName
		}
		return strings.ToLower(a) < strings.ToLower(b)
	})
	if contacts == nil {
		contacts = []contactRow{}
	}
	writeJSON(w, map[string]any{"contacts": contacts})
}

func handleGet(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	id := r.URL.Query().Get("id")
	c, err := parseVCard(filepath.Join(contactsDir(uid), id+".vcf"))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	writeJSON(w, c)
}

func handleCreate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var c Contact
	json.NewDecoder(r.Body).Decode(&c)
	c.ID = fmt.Sprintf("%d", time.Now().UnixNano())
	os.WriteFile(filepath.Join(contactsDir(userID(r)), c.ID+".vcf"), []byte(toVCard(c)), 0644)
	writeJSON(w, c)
}

func handleUpdate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var c Contact
	json.NewDecoder(r.Body).Decode(&c)
	if c.ID == "" {
		http.Error(w, "missing id", 400)
		return
	}
	os.WriteFile(filepath.Join(contactsDir(userID(r)), c.ID+".vcf"), []byte(toVCard(c)), 0644)
	writeJSON(w, c)
}

func handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	id := r.URL.Query().Get("id")
	os.Remove(filepath.Join(contactsDir(uid), id+".vcf"))
	writeJSON(w, map[string]string{"status": "ok"})
}

func handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	q := strings.ToLower(r.URL.Query().Get("q"))
	dir := contactsDir(uid)
	entries, _ := os.ReadDir(dir)
	var results []Contact
	for _, e := range entries {
		if !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		c, err := parseVCard(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		if strings.Contains(strings.ToLower(c.FirstName+" "+c.LastName+" "+c.Email+" "+c.Phone), q) {
			results = append(results, c)
		}
	}
	if results == nil {
		results = []Contact{}
	}
	writeJSON(w, map[string]any{"contacts": results})
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
