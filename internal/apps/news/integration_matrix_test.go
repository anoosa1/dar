package news

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type newsSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *newsSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func auxRequest(t *testing.T, app *App, user *auth.User, method, target, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, target, strings.NewReader(body))
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
	rec := httptest.NewRecorder()
	app.handleAux(rec, req)
	return rec
}

func TestNewsCompatibilityStorageAuxSearchAndLifecycle(t *testing.T) {
	app := testApp(t)
	user := &auth.User{ID: "alice", DisplayName: "Alice"}

	rec := apiRequest(t, app, user, http.MethodPost, v13Base+"/folders", `{"name":"Engineering"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("create folder: %d %s", rec.Code, rec.Body.String())
	}
	var createdFolders struct {
		Folders []Folder `json:"folders"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &createdFolders); err != nil || len(createdFolders.Folders) != 1 {
		t.Fatalf("folder response = %s, %v", rec.Body.String(), err)
	}
	folder := createdFolders.Folders[0]

	feed, err := app.insertFeed(context.Background(), user.ID, Feed{
		URL: "https://example.test/feed.xml", Link: "https://example.test/", Title: "Engineering News", FolderID: folder.ID,
	})
	if err != nil {
		t.Fatal(err)
	}
	var itemIDs []int64
	err = app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		for index, title := range []string{"First article", ""} {
			now := time.Now().Unix() + int64(index)
			if err := sqlitex.Execute(conn, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,url,title,author,pub_date,body,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{
				Args: []any{user.ID, feed.ID, fmt.Sprintf("guid-%d", index), fmt.Sprintf("hash-%d", index), "https://example.test/article", title, "Reporter", now, "<p>Searchable regression body</p>", 1, now, fmt.Sprintf("fingerprint-%d", index)},
			}); err != nil {
				return err
			}
			itemIDs = append(itemIDs, conn.LastInsertRowID())
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}

	v13Cases := []struct {
		method string
		path   string
		body   string
		status int
	}{
		{http.MethodGet, "/folders", "", 200},
		{http.MethodPut, "/folders/" + strconv.FormatInt(folder.ID, 10), `{"name":"Platform"}`, 200},
		{http.MethodGet, "/feeds", "", 200},
		{http.MethodPost, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/rename", `{"feedTitle":"Platform News"}`, 200},
		{http.MethodPost, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/move", fmt.Sprintf(`{"folderId":%d}`, folder.ID), 200},
		{http.MethodGet, "/items?type=0&id=" + strconv.FormatInt(feed.ID, 10) + "&getRead=true&oldestFirst=true", "", 200},
		{http.MethodGet, "/items/updated?type=3&lastModified=1", "", 200},
		{http.MethodPost, "/items/" + strconv.FormatInt(itemIDs[0], 10) + "/star", "", 200},
		{http.MethodPost, "/items/" + strconv.FormatInt(itemIDs[0], 10) + "/unstar", "", 200},
		{http.MethodPost, "/items/unread/multiple", fmt.Sprintf(`{"itemIds":[%d,%d]}`, itemIDs[0], itemIDs[1]), 200},
		{http.MethodPost, "/items/read/multiple", fmt.Sprintf(`{"items":[%d,%d]}`, itemIDs[0], itemIDs[1]), 200},
		{http.MethodPost, "/items/starred/multiple", fmt.Sprintf(`{"itemIds":[%d]}`, itemIDs[0]), 200},
		{http.MethodPost, "/items/unstarred/multiple", fmt.Sprintf(`{"itemIds":[%d]}`, itemIDs[0]), 200},
		{http.MethodPost, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/read", `{"newestItemId":0}`, 200},
		{http.MethodPost, "/folders/" + strconv.FormatInt(folder.ID, 10) + "/read", `{"newestItemId":0}`, 200},
		{http.MethodPost, "/items/read", `{"newestItemId":0}`, 200},
	}
	for _, tc := range v13Cases {
		rec = apiRequest(t, app, user, tc.method, v13Base+tc.path, tc.body)
		if rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.path, rec.Code, tc.status, rec.Body.String())
		}
	}

	prefs := defaultPreferences()
	prefs.ShowAll = true
	prefJSON, _ := json.Marshal(prefs)
	for _, tc := range []struct {
		method string
		path   string
		body   string
		status int
	}{
		{http.MethodPut, "/news/api/preferences", string(prefJSON), 200},
		{http.MethodGet, "/news/api/preferences", "", 200},
		{http.MethodPatch, "/news/api/feed/" + strconv.FormatInt(feed.ID, 10), `{"pinned":true,"ordering":2,"keywordFilters":"go"}`, 200},
		{http.MethodPost, "/news/api/items/" + strconv.FormatInt(itemIDs[0], 10) + "/view", "", 200},
		{http.MethodGet, "/news/api/recent", "", 200},
		{http.MethodGet, "/news/api/opml", "", 200},
		{http.MethodGet, "/news/api/articles", "", 200},
		{http.MethodGet, "/news/api/content/" + strconv.FormatInt(itemIDs[0], 10), "", 200},
	} {
		rec = auxRequest(t, app, user, tc.method, tc.path, tc.body)
		if rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.path, rec.Code, tc.status, rec.Body.String())
		}
	}

	sink := &newsSearchSink{}
	if err = app.IndexSearch(context.Background(), user.ID, sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) != 2 || sink.docs[1].Title != "Untitled article" || strings.Contains(sink.docs[0].Body, "<p>") {
		t.Fatalf("search docs = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err = app.IndexSearch(context.Background(), user.ID, sink); !errors.Is(err, context.Canceled) {
		t.Fatalf("search sink error = %v", err)
	}
	if nonEmptyNewsTitle(" ") != "Untitled article" || nonEmptyNewsTitle("Title") != "Title" {
		t.Fatal("news title fallback failed")
	}

	admin := &auth.User{ID: "admin", IsAdmin: true}
	for _, path := range []string{"/status", "/user", "/feeds/all", "/cleanup/before-update", "/cleanup/after-update"} {
		rec = apiRequest(t, app, admin, http.MethodGet, v13Base+path, "")
		if rec.Code != http.StatusOK {
			t.Fatalf("admin %s: %d %s", path, rec.Code, rec.Body.String())
		}
	}
	if rec = apiRequest(t, app, user, http.MethodGet, v13Base+"/feeds/all", ""); rec.Code != http.StatusForbidden {
		t.Fatalf("non-admin feeds/all = %d", rec.Code)
	}

	if err = app.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	shutdownCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err = app.Shutdown(shutdownCtx); err != nil {
		t.Fatal(err)
	}
	if err = New(Config{}).Start(context.Background()); err == nil {
		t.Fatal("news started without a database")
	}

	rec = apiRequest(t, app, user, http.MethodDelete, v13Base+"/feeds/"+strconv.FormatInt(feed.ID, 10), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete feed: %d %s", rec.Code, rec.Body.String())
	}
	rec = apiRequest(t, app, user, http.MethodDelete, v13Base+"/folders/"+strconv.FormatInt(folder.ID, 10), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete folder: %d %s", rec.Code, rec.Body.String())
	}
	if _, err = app.feed(context.Background(), user.ID, feed.ID); !errors.Is(err, errNotFound) {
		t.Fatalf("deleted feed error = %v", err)
	}
}

func TestNewsCompatibilityValidationMatrix(t *testing.T) {
	app := testApp(t)
	user := &auth.User{ID: "alice"}
	for _, tc := range []struct {
		method string
		path   string
		body   string
		status int
	}{
		{http.MethodGet, "/folders/not-a-number", "", 422},
		{http.MethodPost, "/folders", `{`, 422},
		{http.MethodPost, "/folders", `{"name":""}`, 422},
		{http.MethodDelete, "/folders/999", "", 404},
		{http.MethodDelete, "/feeds/999", "", 404},
		{http.MethodPost, "/items/nope/read", "", 422},
		{http.MethodPost, "/items/nope/multiple", `{}`, 404},
		{http.MethodPatch, "/feeds", "", 405},
	} {
		rec := apiRequest(t, app, user, tc.method, v13Base+tc.path, tc.body)
		if rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.path, rec.Code, tc.status, rec.Body.String())
		}
	}
}

func TestNewsHermeticDiscoveryRefreshAndFailureRecovery(t *testing.T) {
	var baseURL string
	fixture := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			w.Header().Set("Content-Type", "text/html")
			fmt.Fprintf(w, `<html><head><link rel="alternate" type="application/rss+xml" href="%s/feed.xml"></head></html>`, baseURL)
		case "/feed.xml":
			w.Header().Set("Content-Type", "application/rss+xml")
			fmt.Fprintf(w, `<?xml version="1.0"?><rss version="2.0"><channel><title>Fixture Feed</title><link>%s</link><image><url>%s/icon.png</url><title>icon</title><link>%s</link></image>
				<item><guid>one</guid><title>Kept item</title><link>%s/article</link><description>summary</description><pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate><enclosure url="%s/audio.mp3" type="audio/mpeg"/></item>
				<item><guid>two</guid><title>Filtered sponsor item</title><description>hidden</description></item>
			</channel></rss>`, baseURL, baseURL, baseURL, baseURL, baseURL)
		case "/article":
			w.Header().Set("Content-Type", "text/html")
			fmt.Fprint(w, `<html><body><main><p>Full article body</p></main></body></html>`)
		case "/icon.png":
			w.Header().Set("Content-Type", "image/png")
			_, _ = w.Write([]byte{0x89, 0x50, 0x4e, 0x47})
		case "/too-large":
			w.Write(make([]byte, 65))
		default:
			http.Error(w, "fixture failure", http.StatusBadGateway)
		}
	}))
	defer fixture.Close()
	baseURL = fixture.URL

	app := testApp(t)
	user := &auth.User{ID: "fixture-user"}
	rec := apiRequest(t, app, user, http.MethodPost, v13Base+"/feeds", fmt.Sprintf(`{"url":%q,"autoDiscover":true}`, baseURL+"/"))
	if rec.Code != http.StatusOK {
		t.Fatalf("discover feed: %d %s", rec.Code, rec.Body.String())
	}
	var response struct {
		Feeds []Feed `json:"feeds"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &response); err != nil || len(response.Feeds) != 1 {
		t.Fatalf("feed response = %s, %v", rec.Body.String(), err)
	}
	feed := response.Feeds[0]
	if feed.Title != "Fixture Feed" || feed.ArticlesPerUpdate != 2 {
		t.Fatalf("discovered feed = %#v", feed)
	}

	if err := app.patchFeed(context.Background(), user.ID, feed.ID, map[string]any{
		"fullTextEnabled": true,
		"keywordFilters":  "sponsor",
		"updateMode":      1,
	}); err != nil {
		t.Fatal(err)
	}
	if err := app.updateFeed(context.Background(), user.ID, feed.ID); err != nil {
		t.Fatal(err)
	}
	items, err := app.items(context.Background(), user.ID, itemQuery{Type: 3, GetRead: true, BatchSize: -1})
	if err != nil || len(items) != 2 {
		t.Fatalf("items = %#v, %v", items, err)
	}
	foundFullText := false
	for _, item := range items {
		if item.GUID == "one" && strings.Contains(item.Body, "Full article body") {
			foundFullText = true
		}
	}
	if !foundFullText {
		t.Fatalf("full text was not refreshed: %#v", items)
	}

	rec = apiRequest(t, app, user, http.MethodGet, v13Base+"/favicon/"+hashMD5(feed.URL), "")
	if rec.Code != http.StatusOK || rec.Header().Get("Cache-Control") == "" {
		t.Fatalf("favicon: %d headers=%v body=%x", rec.Code, rec.Header(), rec.Body.Bytes())
	}
	if _, _, _, err = app.fetchBytes(context.Background(), baseURL+"/too-large", 32, "", ""); err == nil {
		t.Fatal("oversized response accepted")
	}
	if _, _, _, err = app.fetchBytes(context.Background(), baseURL+"/failure", 32, "", ""); err == nil {
		t.Fatal("HTTP failure accepted")
	}
	if _, _, err = app.resolveSubscription(context.Background(), baseURL+"/article", "", "", true); err == nil {
		t.Fatal("HTML without feed discovery accepted")
	}
	opml := `<?xml version="1.0"?><opml version="2.0"><body><outline text="Imported"><outline type="rss" text="Fixture" xmlUrl="` + baseURL + `/feed.xml?opml=1"/></outline></body></opml>`
	rec = auxRequest(t, app, &auth.User{ID: "opml-user"}, http.MethodPost, "/news/api/opml", opml)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"created":1`) {
		t.Fatalf("OPML import: %d %s", rec.Code, rec.Body.String())
	}
	rec = auxRequest(t, app, &auth.User{ID: "opml-user"}, http.MethodGet, "/news/api/opml", "")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Fixture") {
		t.Fatalf("OPML export: %d %s", rec.Code, rec.Body.String())
	}

	broken, err := app.insertFeed(context.Background(), user.ID, Feed{URL: baseURL + "/failure", Title: "Broken"})
	if err != nil {
		t.Fatal(err)
	}
	if err = app.updateFeed(context.Background(), user.ID, broken.ID); err == nil {
		t.Fatal("broken feed refresh succeeded")
	}
	broken, err = app.feed(context.Background(), user.ID, broken.ID)
	if err != nil || broken.UpdateErrorCount != 1 || broken.LastUpdateError == "" {
		t.Fatalf("broken feed state = %#v, %v", broken, err)
	}

	app.refreshUser(context.Background(), user.ID)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	app.runJobs(ctx, user.ID, []int64{feed.ID, broken.ID})
}
