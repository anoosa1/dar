package news

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func apiRequest(t *testing.T, a *App, user *auth.User, method, path, body string) *httptest.ResponseRecorder {
	t.Helper()
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	if body != "" {
		r.Header.Set("Content-Type", "application/json")
	}
	if user != nil {
		r = r.WithContext(context.WithValue(r.Context(), server.UserContextKey, user))
	}
	w := httptest.NewRecorder()
	a.handleV13(w, r)
	return w
}

func TestV13VersionAndCORS(t *testing.T) {
	a := testApp(t)
	user := &auth.User{ID: "alice"}
	w := apiRequest(t, a, user, http.MethodGet, v13Base+"/version", "")
	if w.Code != 200 {
		t.Fatalf("status=%d body=%s", w.Code, w.Body.String())
	}
	if w.Header().Get("Access-Control-Allow-Origin") != "*" {
		t.Fatal("missing CORS")
	}
	var got map[string]string
	if json.Unmarshal(w.Body.Bytes(), &got) != nil || got["version"] != compatibilityVersion {
		t.Fatalf("body=%s", w.Body.String())
	}
	preflight := apiRequest(t, a, nil, http.MethodOptions, v13Base+"/feeds", "")
	if preflight.Code != http.StatusNoContent {
		t.Fatalf("preflight=%d", preflight.Code)
	}
	if preflight.Header().Get("Access-Control-Allow-Credentials") != "" {
		t.Fatal("cross-origin credentials enabled")
	}
}

func TestNewsTemplateLoadsAndTracksSearchDeepLinks(t *testing.T) {
	data, err := os.ReadFile("../../webui/templates/news.html")
	if err != nil {
		t.Fatal(err)
	}
	page := string(data)
	for _, want := range []string{
		"await loadArticles(targetID)",
		"p.set('offset',String(targetID))",
		"window.addEventListener('hashchange'",
	} {
		if !strings.Contains(page, want) {
			t.Fatalf("news search deep-link handling is missing %q", want)
		}
	}
}

func TestV13FolderFeedAndItemState(t *testing.T) {
	a := testApp(t)
	user := &auth.User{ID: "alice"}
	folder, err := a.createFolder(context.Background(), user.ID, "Tech")
	if err != nil {
		t.Fatal(err)
	}
	feed, err := a.insertFeed(context.Background(), user.ID, Feed{URL: "https://example.com/rss", Title: "Example", FolderID: folder.ID})
	if err != nil {
		t.Fatal(err)
	}
	var itemID int64
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,title,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{user.ID, feed.ID, "one", "hash", "Article", 1, time.Now().Unix(), "fingerprint"}}); err != nil {
			return err
		}
		itemID = c.LastInsertRowID()
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	w := apiRequest(t, a, user, http.MethodGet, v13Base+"/items?type=3&getRead=false&batchSize=-1", "")
	if w.Code != 200 || !strings.Contains(w.Body.String(), `"title":"Article"`) {
		t.Fatalf("list status=%d body=%s", w.Code, w.Body.String())
	}
	w = apiRequest(t, a, user, http.MethodPost, v13Base+"/items/"+strconv.FormatInt(itemID, 10)+"/read", "")
	if w.Code != 200 {
		t.Fatalf("read status=%d", w.Code)
	}
	items, err := a.items(context.Background(), user.ID, itemQuery{Type: 3, GetRead: true, BatchSize: -1})
	if err != nil || len(items) != 1 || items[0].Unread {
		t.Fatalf("items=%+v err=%v", items, err)
	}
}

func TestArticleContentIsUserScopedAndScriptless(t *testing.T) {
	a := testApp(t)
	feed, err := a.insertFeed(context.Background(), "alice", Feed{URL: "https://example.com/rss", Title: "Example"})
	if err != nil {
		t.Fatal(err)
	}
	malicious := `<script>top.location='https://evil.example'</script><img src="https://tracker.example/pixel"><form action="https://evil.example"><button>go</button></form>`
	var itemID int64
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,title,body,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{"alice", feed.ID, "malicious", "malicious-hash", "Unsafe", malicious, 1, time.Now().Unix(), "malicious-fingerprint"}}); err != nil {
			return err
		}
		itemID = c.LastInsertRowID()
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest(http.MethodGet, "/news/api/content/"+strconv.FormatInt(itemID, 10), nil)
	w := httptest.NewRecorder()
	a.articleContent(w, r, "alice", strconv.FormatInt(itemID, 10))
	if w.Code != http.StatusOK || !strings.Contains(w.Body.String(), malicious) {
		t.Fatalf("content status=%d body=%s", w.Code, w.Body.String())
	}
	for _, want := range []string{`name="theme-color" content="#1e1e2e"`, `background:#1e1e2e`, `color:#cdd6f4`} {
		if !strings.Contains(w.Body.String(), want) {
			t.Errorf("themed article body does not contain %q", want)
		}
	}
	csp := w.Header().Get("Content-Security-Policy")
	for _, directive := range []string{"script-src 'none'", "img-src 'none'", "form-action 'none'", "frame-ancestors 'self'"} {
		if !strings.Contains(csp, directive) {
			t.Fatalf("CSP %q missing %q", csp, directive)
		}
	}
	w = httptest.NewRecorder()
	a.articleContent(w, r, "bob", strconv.FormatInt(itemID, 10))
	if w.Code != http.StatusNotFound {
		t.Fatalf("cross-user content status=%d", w.Code)
	}
	r = httptest.NewRequest(http.MethodGet, "/news/api/content/"+strconv.FormatInt(itemID, 10)+"?images=1", nil)
	w = httptest.NewRecorder()
	a.articleContent(w, r, "alice", strconv.FormatInt(itemID, 10))
	if !strings.Contains(w.Header().Get("Content-Security-Policy"), "img-src https: http: data:") {
		t.Fatalf("explicit image policy=%q", w.Header().Get("Content-Security-Policy"))
	}
}

func TestArticleContentRendersRequestedOdyseeVideoEnclosure(t *testing.T) {
	a := testApp(t)
	feed, err := a.insertFeed(context.Background(), "alice", Feed{URL: "https://odysee.com/$/rss/@Odysee:8", Title: "Odysee"})
	if err != nil {
		t.Fatal(err)
	}
	const videoURL = "https://odysee.com/$/rss/media/channel-roundup/claim/video.mp4?download=1&source=rss"
	var itemID int64
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,title,body,enclosure_mime,enclosure_link,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{"alice", feed.ID, "video", "video-hash", "Video", "", "video/mp4", videoURL, 1, time.Now().Unix(), "video-fingerprint"}}); err != nil {
			return err
		}
		itemID = c.LastInsertRowID()
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	path := "/news/api/content/" + strconv.FormatInt(itemID, 10)
	r := httptest.NewRequest(http.MethodGet, path, nil)
	w := httptest.NewRecorder()
	a.articleContent(w, r, "alice", strconv.FormatInt(itemID, 10))
	if w.Code != http.StatusNotFound {
		t.Fatalf("unrequested enclosure status=%d body=%s", w.Code, w.Body.String())
	}

	r = httptest.NewRequest(http.MethodGet, path+"?enclosures=1", nil)
	w = httptest.NewRecorder()
	a.articleContent(w, r, "alice", strconv.FormatInt(itemID, 10))
	if w.Code != http.StatusOK {
		t.Fatalf("requested enclosure status=%d body=%s", w.Code, w.Body.String())
	}
	for _, want := range []string{`<video controls playsinline preload="metadata"`, `src="https://odysee.com/$/rss/media/channel-roundup/claim/video.mp4?download=1&amp;source=rss"`, `media-src https: http:`} {
		got := w.Body.String()
		if strings.HasPrefix(want, "media-src") {
			got = w.Header().Get("Content-Security-Policy")
		}
		if !strings.Contains(got, want) {
			t.Errorf("response does not contain %q: %s", want, got)
		}
	}
	if strings.Contains(w.Header().Get("Content-Security-Policy"), "script-src https:") {
		t.Fatal("enclosure playback enabled scripts")
	}
}

func TestArticleContentRendersScriptlessYouTubeFallback(t *testing.T) {
	a := testApp(t)
	feed, err := a.insertFeed(context.Background(), "alice", Feed{URL: "https://www.youtube.com/feeds/videos.xml?channel_id=channel", Title: "Channel"})
	if err != nil {
		t.Fatal(err)
	}
	const (
		title    = "Building My Endgame Keyboard from Scratch"
		videoURL = "https://www.youtube.com/watch?v=rvM2BthjEI4"
	)
	var itemID int64
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,url,title,body,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{"alice", feed.ID, "yt:video:rvM2BthjEI4", "youtube-hash", videoURL, title, "", 1, time.Now().Unix(), "youtube-fingerprint"}}); err != nil {
			return err
		}
		itemID = c.LastInsertRowID()
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest(http.MethodGet, "/news/api/content/"+strconv.FormatInt(itemID, 10), nil)
	w := httptest.NewRecorder()
	a.articleContent(w, r, "alice", strconv.FormatInt(itemID, 10))
	if w.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", w.Code, w.Body.String())
	}
	for _, want := range []string{
		`class="youtube-fallback"`,
		`src="https://i.ytimg.com/vi/rvM2BthjEI4/hqdefault.jpg"`,
		`alt="Preview for Building My Endgame Keyboard from Scratch"`,
		`Use <strong>Watch on YouTube</strong> above`,
	} {
		if !strings.Contains(w.Body.String(), want) {
			t.Errorf("fallback does not contain %q: %s", want, w.Body.String())
		}
	}
	csp := w.Header().Get("Content-Security-Policy")
	for _, want := range []string{"script-src 'none'", "frame-src 'none'", "img-src https://i.ytimg.com"} {
		if !strings.Contains(csp, want) {
			t.Errorf("CSP %q does not contain %q", csp, want)
		}
	}
	if strings.Contains(w.Body.String(), "<iframe") || strings.Contains(w.Body.String(), "<script") {
		t.Fatal("YouTube fallback contains executable or framed content")
	}
}

func TestYouTubeVideoIDRejectsLookalikeAndMalformedURLs(t *testing.T) {
	valid := map[string]string{
		"https://www.youtube.com/watch?v=rvM2BthjEI4": "rvM2BthjEI4",
		"https://youtu.be/rvM2BthjEI4":                "rvM2BthjEI4",
		"https://youtube.com/shorts/rvM2BthjEI4":      "rvM2BthjEI4",
	}
	for raw, want := range valid {
		if got := youtubeVideoID(raw); got != want {
			t.Errorf("youtubeVideoID(%q)=%q, want %q", raw, got, want)
		}
	}
	for _, raw := range []string{
		"https://youtube.com.evil.example/watch?v=rvM2BthjEI4",
		"javascript:alert(1)",
		"https://www.youtube.com/watch?v=too-short",
		"https://youtu.be/rvM2BthjEI4/extra",
	} {
		if got := youtubeVideoID(raw); got != "" {
			t.Errorf("youtubeVideoID(%q)=%q", raw, got)
		}
	}
}

func TestEnclosureHTMLRejectsUnsafeAndNonMediaURLs(t *testing.T) {
	for _, test := range []struct {
		mime, url string
	}{
		{"video/mp4", "javascript:alert(1)"},
		{"video/mp4", "//odysee.com/video.mp4"},
		{"text/html", "https://odysee.com/video"},
	} {
		if got := enclosureHTML(test.mime, test.url); got != "" {
			t.Errorf("enclosureHTML(%q, %q)=%q", test.mime, test.url, got)
		}
	}
}

func TestV13AdminUpdaterAuthorization(t *testing.T) {
	a := testApp(t)
	_, _ = a.insertFeed(context.Background(), "alice", Feed{URL: "https://example.com/rss", Title: "Example"})
	ordinary := apiRequest(t, a, &auth.User{ID: "alice"}, http.MethodGet, v13Base+"/feeds/all", "")
	if ordinary.Code != 403 {
		t.Fatalf("ordinary status=%d", ordinary.Code)
	}
	admin := apiRequest(t, a, &auth.User{ID: "admin", IsAdmin: true}, http.MethodGet, v13Base+"/feeds/all", "")
	if admin.Code != 200 || !strings.Contains(admin.Body.String(), `"userId":"alice"`) {
		t.Fatalf("admin status=%d body=%s", admin.Code, admin.Body.String())
	}
	cleanup := apiRequest(t, a, &auth.User{ID: "admin", IsAdmin: true}, http.MethodGet, v13Base+"/cleanup/after-update", "")
	if cleanup.Code != 200 {
		t.Fatalf("cleanup status=%d", cleanup.Code)
	}
}

func TestImportNextcloudArticleArrayByFeedLink(t *testing.T) {
	a := testApp(t)
	user := &auth.User{ID: "alice"}
	feed, err := a.insertFeed(context.Background(), user.ID, Feed{URL: "https://example.com/rss", Link: "https://example.com/site/", Title: "Example"})
	if err != nil {
		t.Fatal(err)
	}
	body := `[{"guid":"article-one","url":"https://example.com/article","title":"Imported","author":null,"pubDate":1700000000,"body":"<p>Hello</p>","unread":true,"starred":true,"feedLink":"https://EXAMPLE.com/site","rtl":false}]`
	r := httptest.NewRequest(http.MethodPost, "/news/api/articles", strings.NewReader(body))
	r = r.WithContext(context.WithValue(r.Context(), server.UserContextKey, user))
	w := httptest.NewRecorder()
	a.handleAux(w, r)
	if w.Code != http.StatusOK || !strings.Contains(w.Body.String(), `"imported":1`) {
		t.Fatalf("status=%d body=%s", w.Code, w.Body.String())
	}
	items, err := a.items(context.Background(), user.ID, itemQuery{Type: 0, ID: feed.ID, GetRead: true, BatchSize: -1})
	if err != nil || len(items) != 1 {
		t.Fatalf("items=%+v err=%v", items, err)
	}
	if items[0].GUIDHash == "" || items[0].Fingerprint == "" || !items[0].Unread || !items[0].Starred {
		t.Fatalf("imported item=%+v", items[0])
	}
}

func TestDecodeArticleArchiveRejectsWrongShape(t *testing.T) {
	if _, err := decodeArticleArchive([]byte(`{"feeds":[]}`)); err == nil {
		t.Fatal("accepted non-article archive")
	}
}
