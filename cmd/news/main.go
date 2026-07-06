// News daemon — RSS reader with REST API.
package main

import (
	"encoding/json"
	"encoding/xml"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

var (
	port     = flag.Int("port", 8087, "HTTP port")
	dataRoot = flag.String("data-root", "data", "Root directory for user data")
	refreshMin = flag.Int("refresh", 30, "Feed refresh interval in minutes")
)

var mu sync.Mutex

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/feeds", handleFeeds)
	mux.HandleFunc("/feed", handleFeed)
	mux.HandleFunc("/articles", handleArticles)
	mux.HandleFunc("/refresh", handleRefresh)
	mux.HandleFunc("/health", handleHealth)
	go refreshLoop()
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[news] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Feed struct {
	ID    string `json:"id"`
	Title string `json:"title"`
	URL   string `json:"url"`
}

type Article struct {
	ID      string `json:"id"`
	FeedID  string `json:"feed_id"`
	Title   string `json:"title"`
	Link    string `json:"link"`
	Summary string `json:"summary"`
	Date    string `json:"date"`
	Read    bool   `json:"read"`
	Starred bool   `json:"starred"`
}

type rssFeed struct {
	XMLName xml.Name `xml:"rss"`
	Channel struct {
		Title string `xml:"title"`
		Items []struct {
			Title       string `xml:"title"`
			Link        string `xml:"link"`
			Description string `xml:"description"`
			PubDate     string `xml:"pubDate"`
		} `xml:"item"`
	} `xml:"channel"`
}

func newsDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "news")
	os.MkdirAll(d, 0755)
	return d
}

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func loadFeeds(uid string) []Feed {
	data, _ := os.ReadFile(filepath.Join(newsDir(uid), "feeds.json"))
	var f []Feed
	json.Unmarshal(data, &f)
	if f == nil { f = []Feed{} }
	return f
}

func saveFeeds(uid string, f []Feed) {
	data, _ := json.Marshal(f)
	os.WriteFile(filepath.Join(newsDir(uid), "feeds.json"), data, 0644)
}

func loadArticles(uid string) []Article {
	data, _ := os.ReadFile(filepath.Join(newsDir(uid), "articles.json"))
	var a []Article
	json.Unmarshal(data, &a)
	if a == nil { a = []Article{} }
	return a
}

func saveArticles(uid string, a []Article) {
	data, _ := json.Marshal(a)
	os.WriteFile(filepath.Join(newsDir(uid), "articles.json"), data, 0644)
}

func handleFeeds(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	writeJSON(w, map[string]any{"feeds": loadFeeds(uid)})
}

func handleFeed(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	mu.Lock()
	defer mu.Unlock()
	switch r.Method {
	case "POST":
		var f Feed
		json.NewDecoder(r.Body).Decode(&f)
		f.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		// Fetch feed metadata
		title, _ := fetchFeedTitle(f.URL)
		if title == "" { title = f.URL }
		f.Title = title
		feeds := loadFeeds(uid)
		feeds = append(feeds, f)
		saveFeeds(uid, feeds)
		// Fetch initial articles
		fetchArticles(uid, f)
		writeJSON(w, f)
	case "DELETE":
		id := r.URL.Query().Get("id")
		feeds := loadFeeds(uid)
		for i, f := range feeds {
			if f.ID == id {
				feeds = append(feeds[:i], feeds[i+1:]...)
				saveFeeds(uid, feeds)
				break
			}
		}
		// Remove articles
		articles := loadArticles(uid)
		var remaining []Article
		for _, a := range articles {
			if a.FeedID != id { remaining = append(remaining, a) }
		}
		saveArticles(uid, remaining)
		writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleArticles(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	feedID := r.URL.Query().Get("feed_id")
	unread := r.URL.Query().Get("unread")
	articles := loadArticles(uid)
	var result []Article
	for _, a := range articles {
		if feedID != "" && a.FeedID != feedID { continue }
		if unread == "true" && a.Read { continue }
		result = append(result, a)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].Date > result[j].Date })
	// Toggle read/starred via query params
	if id := r.URL.Query().Get("read"); id != "" {
		toggleArticle(uid, id, func(a *Article) { a.Read = true })
		http.Redirect(w, r, r.Referer(), 302)
		return
	}
	if id := r.URL.Query().Get("star"); id != "" {
		toggleArticle(uid, id, func(a *Article) { a.Starred = !a.Starred })
	}
	writeJSON(w, map[string]any{"articles": result})
}

func toggleArticle(uid, id string, fn func(*Article)) {
	mu.Lock()
	defer mu.Unlock()
	articles := loadArticles(uid)
	for i := range articles {
		if articles[i].ID == id {
			fn(&articles[i])
			saveArticles(uid, articles)
			return
		}
	}
}

func handleRefresh(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	go func() {
		for _, f := range loadFeeds(uid) {
			fetchArticles(uid, f)
		}
	}()
	writeJSON(w, map[string]string{"status": "refreshing"})
}

func fetchFeedTitle(url string) (string, error) {
	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Get(url)
	if err != nil { return "", err }
	defer resp.Body.Close()
	data, _ := io.ReadAll(io.LimitReader(resp.Body, 256*1024))
	var f rssFeed
	if err := xml.Unmarshal(data, &f); err != nil { return "", err }
	return f.Channel.Title, nil
}

func fetchArticles(uid string, feed Feed) {
	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Get(feed.URL)
	if err != nil { log.Printf("[news] fetch %s: %v", feed.Title, err); return }
	defer resp.Body.Close()
	data, _ := io.ReadAll(io.LimitReader(resp.Body, 256*1024))
	var f rssFeed
	if err := xml.Unmarshal(data, &f); err != nil { log.Printf("[news] parse %s: %v", feed.Title, err); return }
	existing := loadArticles(uid)
	for _, item := range f.Channel.Items {
		// Deduplicate by link
		dup := false
		for _, a := range existing {
			if a.Link == item.Link { dup = true; break }
		}
		if dup { continue }
		existing = append(existing, Article{
			ID:      fmt.Sprintf("%d", time.Now().UnixNano()),
			FeedID:  feed.ID,
			Title:   item.Title,
			Link:    item.Link,
			Summary: stripHTML(item.Description),
			Date:    item.PubDate,
		})
	}
	saveArticles(uid, existing)
}

func stripHTML(s string) string {
	inTag := false
	var b strings.Builder
	for _, c := range s {
		if c == '<' { inTag = true; continue }
		if c == '>' { inTag = false; continue }
		if !inTag { b.WriteRune(c) }
	}
	result := strings.TrimSpace(b.String())
	if len(result) > 300 { result = result[:300] + "..." }
	return result
}

func refreshLoop() {
	for {
		time.Sleep(time.Duration(*refreshMin) * time.Minute)
		dir := filepath.Join(*dataRoot)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !e.IsDir() { continue }
			feeds := loadFeeds(e.Name())
			for _, f := range feeds { fetchArticles(e.Name(), f) }
		}
	}
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
