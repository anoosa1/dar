package news

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func TestNewsNumericConversionMatrix(t *testing.T) {
	for _, tc := range []struct {
		value any
		want  int64
		ok    bool
	}{
		{float64(3.9), 3, true},
		{int64(4), 4, true},
		{int(5), 5, true},
		{"6", 0, false},
	} {
		got, ok := asInt64(tc.value)
		if got != tc.want || ok != tc.ok {
			t.Errorf("asInt64(%T(%v))=%d,%v", tc.value, tc.value, got, ok)
		}
	}
}

func TestNewsArticleArchiveImportExportSkipAndDuplicateMatrix(t *testing.T) {
	a := testApp(t)
	feed, err := a.insertFeed(context.Background(), "alice", Feed{URL: "https://Example.Test/feed/", Link: "https://example.test/site", Title: "Archive"})
	if err != nil {
		t.Fatal(err)
	}
	body := `[
		{"feedLink":"https://example.test/feed","guid":"archive-one","url":"https://example.test/one","title":"Imported","body":"Body","unread":true},
		{"feedLink":"https://missing.test/feed","guid":"skipped","title":"Skipped"},
		{"feedId":` + strconv.FormatInt(feed.ID, 10) + `,"guid":"archive-one","title":"Duplicate"}
	]`
	rec := httptest.NewRecorder()
	a.articleArchiveAPI(rec, newsRequest(http.MethodPost, "/news/api/articles", body, "alice"), "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"imported":1`) ||
		!strings.Contains(rec.Body.String(), `"skipped":1`) || !strings.Contains(rec.Body.String(), `"duplicates":1`) {
		t.Fatalf("article import status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = httptest.NewRecorder()
	a.articleArchiveAPI(rec, newsRequest(http.MethodGet, "/news/api/articles", "", "alice"), "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "archive-one") ||
		!strings.Contains(rec.Header().Get("Content-Disposition"), "dar-news-articles.json") {
		t.Fatalf("article export status=%d headers=%v body=%s", rec.Code, rec.Header(), rec.Body.String())
	}
	for _, raw := range []string{"", `{}`, `{`, `[]`} {
		rec = httptest.NewRecorder()
		a.articleArchiveAPI(rec, newsRequest(http.MethodPost, "/news/api/articles", raw, "alice"), "alice")
		if raw == "[]" {
			if rec.Code != http.StatusOK {
				t.Errorf("empty array archive status=%d body=%s", rec.Code, rec.Body.String())
			}
		} else if rec.Code != http.StatusUnprocessableEntity {
			t.Errorf("invalid archive %q status=%d body=%s", raw, rec.Code, rec.Body.String())
		}
	}
	rec = httptest.NewRecorder()
	a.articleArchiveAPI(rec, newsRequest(http.MethodPatch, "/news/api/articles", "", "alice"), "alice")
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("article archive method status=%d", rec.Code)
	}
	if canonicalFeedLink("  ") != "" || canonicalFeedLink("not a URL///") != "not a URL" {
		t.Fatal("canonical feed fallback regression")
	}
}

func newsRequest(method, target string, body string, uid string) *http.Request {
	req := httptest.NewRequest(method, target, strings.NewReader(body))
	if uid != "" {
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: uid}))
	}
	return req
}

func TestNewsAdminUpdateRefreshAllAndExplore(t *testing.T) {
	feedServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = w.Write([]byte(`<?xml version="1.0"?><rss version="2.0"><channel><title>Fixture</title><link>https://example.test/</link><item><guid>one</guid><title>First item</title><link>https://example.test/one</link></item></channel></rss>`))
	}))
	defer feedServer.Close()

	a := testApp(t)
	feed, err := a.insertFeed(context.Background(), "alice", Feed{URL: feedServer.URL, Title: "Fixture"})
	if err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		method string
		target string
		status int
	}{
		{http.MethodPost, "/admin/update", http.StatusMethodNotAllowed},
		{http.MethodGet, "/admin/update", http.StatusUnprocessableEntity},
		{http.MethodGet, "/admin/update?userId=alice&feedId=" + strconv.FormatInt(feed.ID, 10), http.StatusOK},
	} {
		rec := httptest.NewRecorder()
		a.adminUpdateFeed(rec, newsRequest(tc.method, tc.target, "", "alice"))
		if rec.Code != tc.status {
			t.Fatalf("%s %s status=%d body=%s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	items, err := a.items(context.Background(), "alice", itemQuery{Type: 0, ID: feed.ID, GetRead: true, BatchSize: 20})
	if err != nil || len(items) != 1 {
		t.Fatalf("updated items=%v err=%v", items, err)
	}

	a.refreshAll(context.Background())
	deadline := time.Now().Add(2 * time.Second)
	for {
		a.mu.Lock()
		running := a.refresh["alice"].Running
		a.mu.Unlock()
		if !running {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("refreshAll did not finish")
		}
		time.Sleep(time.Millisecond)
	}
	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	a.refreshAll(cancelled)

	rec := httptest.NewRecorder()
	a.explore(rec, newsRequest(http.MethodGet, "/explore", "", "alice"))
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Nextcloud News") {
		t.Fatalf("default explore status=%d body=%s", rec.Code, rec.Body.String())
	}

	exploreServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[{"title":"Local","url":"https://example.test/feed"}]`))
	}))
	defer exploreServer.Close()
	a.exploreURL = exploreServer.URL
	rec = httptest.NewRecorder()
	a.explore(rec, newsRequest(http.MethodGet, "/explore", "", "alice"))
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"Local"`) {
		t.Fatalf("remote explore status=%d body=%s", rec.Code, rec.Body.String())
	}

	badExplore := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`not-json`))
	}))
	defer badExplore.Close()
	a.exploreURL = badExplore.URL
	rec = httptest.NewRecorder()
	a.explore(rec, newsRequest(http.MethodGet, "/explore", "", "alice"))
	if rec.Code != http.StatusUnprocessableEntity {
		t.Fatalf("bad explore status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestNewsAuxiliaryValidationAndRefreshStatus(t *testing.T) {
	a := testApp(t)
	user := &auth.User{ID: "alice"}
	feed, err := a.insertFeed(context.Background(), user.ID, Feed{URL: "https://example.test/feed", Title: "Fixture"})
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		method string
		target string
		body   string
		status int
	}{
		{http.MethodPut, "/news/api/preferences", `{`, http.StatusUnprocessableEntity},
		{http.MethodPost, "/news/api/preferences", `{}`, http.StatusNotFound},
		{http.MethodGet, "/news/api/refresh", "", http.StatusOK},
		{http.MethodPost, "/news/api/refresh", "", http.StatusAccepted},
		{http.MethodPatch, "/news/api/refresh", "", http.StatusNotFound},
		{http.MethodPost, "/news/api/recent", "", http.StatusNotFound},
		{http.MethodPost, "/news/api/explore", "", http.StatusNotFound},
		{http.MethodPatch, "/news/api/feed/not-a-number", `{}`, http.StatusUnprocessableEntity},
		{http.MethodPatch, "/news/api/feed/" + strconv.FormatInt(feed.ID, 10), `{`, http.StatusUnprocessableEntity},
		{http.MethodPatch, "/news/api/feed/99999", `{"pinned":true}`, http.StatusNotFound},
		{http.MethodPost, "/news/api/items/not-a-number/view", "", http.StatusUnprocessableEntity},
		{http.MethodPost, "/news/api/items/99999/view", "", http.StatusNotFound},
		{http.MethodGet, "/news/api/content/not-a-number", "", http.StatusNotFound},
		{http.MethodGet, "/news/api/content/99999", "", http.StatusNotFound},
		{http.MethodGet, "/news/api/missing", "", http.StatusNotFound},
	} {
		rec := auxRequest(t, a, user, tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
	if err := a.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
}

func TestNewsClosedDatabaseAPIErrorMatrix(t *testing.T) {
	app := testApp(t)
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	user := &auth.User{ID: "alice", DisplayName: "Alice"}
	cases := []struct {
		method string
		path   string
		body   string
	}{
		{http.MethodGet, "/folders", ""},
		{http.MethodPost, "/folders", `{"name":"Folder"}`},
		{http.MethodPut, "/folders/1", `{"name":"Renamed"}`},
		{http.MethodDelete, "/folders/1", ""},
		{http.MethodPost, "/folders/1/read", `{}`},
		{http.MethodGet, "/feeds", ""},
		{http.MethodDelete, "/feeds/1", ""},
		{http.MethodPost, "/feeds/1/move", `{"folderId":2}`},
		{http.MethodPost, "/feeds/1/rename", `{"feedTitle":"Renamed"}`},
		{http.MethodPost, "/feeds/1/read", `{}`},
		{http.MethodGet, "/items", ""},
		{http.MethodGet, "/items/1", ""},
		{http.MethodPost, "/items/1/read", ""},
		{http.MethodPost, "/items/1/unread", ""},
		{http.MethodPost, "/items/1/star", ""},
		{http.MethodPost, "/items/1/unstar", ""},
		{http.MethodGet, "/favicon/1", ""},
	}
	for _, tc := range cases {
		rec := apiRequest(t, app, user, tc.method, v13Base+tc.path, tc.body)
		if rec.Code < 400 {
			t.Errorf("%s %s unexpectedly returned %d: %s", tc.method, tc.path, rec.Code, rec.Body.String())
		}
	}
	for _, err := range []error{errNotFound, errInvalid, errConflict, errors.New("UNIQUE constraint"), errors.New("storage failed")} {
		rec := httptest.NewRecorder()
		apiError(rec, err)
		if rec.Code < 400 {
			t.Fatalf("error %v returned %d", err, rec.Code)
		}
	}
}

type failingNewsReader struct{}

func (failingNewsReader) Read([]byte) (int, error) { return 0, io.ErrUnexpectedEOF }

func TestNewsArticleArchiveDefaultsDuplicatesAndLimits(t *testing.T) {
	app := testApp(t)
	ctx := context.Background()
	feed, err := app.insertFeed(ctx, "alice", Feed{URL: "https://example.test/feed.xml", Link: "https://example.test/", Title: "Archive"})
	if err != nil {
		t.Fatal(err)
	}
	body := `{"items":[
		{"feedId":` + strconv.FormatInt(feed.ID, 10) + `,"url":"https://example.test/one","title":"One","body":"Body","unread":true},
		{"feedId":` + strconv.FormatInt(feed.ID, 10) + `,"url":"https://example.test/one","title":"One duplicate","body":"Body"},
		{"feedId":` + strconv.FormatInt(feed.ID, 10) + `,"title":"No identity"},
		{"feedLink":"https://missing.example.test/feed","url":"https://example.test/missing"}
	]}`
	req := newsRequest(http.MethodPost, "/api/news/articles", body, "alice")
	rec := httptest.NewRecorder()
	app.articleArchiveAPI(rec, req, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"imported":1`) ||
		!strings.Contains(rec.Body.String(), `"duplicates":1`) || !strings.Contains(rec.Body.String(), `"skipped":2`) {
		t.Fatalf("archive import=%d %s", rec.Code, rec.Body.String())
	}
	rec = httptest.NewRecorder()
	app.articleArchiveAPI(rec, newsRequest(http.MethodGet, "/api/news/articles", "", "alice"), "alice")
	if rec.Code != http.StatusOK || rec.Header().Get("Content-Disposition") == "" {
		t.Fatalf("archive export=%d %s", rec.Code, rec.Body.String())
	}
	rec = httptest.NewRecorder()
	app.articleArchiveAPI(rec, newsRequest(http.MethodDelete, "/api/news/articles", "", "alice"), "alice")
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("archive method=%d", rec.Code)
	}
	if _, err := readLimitedBody(bytes.NewReader([]byte("1234")), 3); err == nil {
		t.Fatal("oversized archive accepted")
	}
	if _, err := readLimitedBody(failingNewsReader{}, 3); err == nil {
		t.Fatal("archive read error ignored")
	}
	value := "value"
	if ptrValue(nil) != nil || ptrValue(&value) != value {
		t.Fatal("ptrValue regression")
	}
}

func TestNewsV13AuthorizationMethodsAndMutationMatrix(t *testing.T) {
	app := testApp(t)
	ctx := context.Background()
	user := &auth.User{ID: "alice", DisplayName: "Alice"}
	admin := &auth.User{ID: "admin", DisplayName: "Admin", IsAdmin: true}
	folder, err := app.createFolder(ctx, user.ID, "Initial")
	if err != nil {
		t.Fatal(err)
	}
	feed, err := app.insertFeed(ctx, user.ID, Feed{URL: "https://example.test/feed", Title: "Initial", FolderID: folder.ID})
	if err != nil {
		t.Fatal(err)
	}
	var itemID int64
	if err := app.withConn(ctx, func(c *sqlite.Conn) error {
		if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,title,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?)`,
			&sqlitex.ExecOptions{Args: []any{user.ID, feed.ID, "matrix", "matrix-hash", "Matrix item", 1, time.Now().Unix(), "matrix-fingerprint"}}); err != nil {
			return err
		}
		itemID = c.LastInsertRowID()
		return nil
	}); err != nil {
		t.Fatal(err)
	}

	call := func(as *auth.User, method, suffix, body string) *httptest.ResponseRecorder {
		return apiRequest(t, app, as, method, v13Base+suffix, body)
	}
	for _, tc := range []struct {
		as     *auth.User
		method string
		path   string
		body   string
		status int
	}{
		{nil, http.MethodGet, "/user", "", http.StatusUnauthorized},
		{user, http.MethodGet, "/user", "", http.StatusOK},
		{user, http.MethodGet, "/status", "", http.StatusOK},
		{user, http.MethodGet, "/cleanup/before-update", "", http.StatusForbidden},
		{admin, http.MethodPost, "/cleanup/before-update", "", http.StatusMethodNotAllowed},
		{admin, http.MethodGet, "/cleanup/after-update", "", http.StatusOK},
		{user, http.MethodGet, "/feeds/all", "", http.StatusForbidden},
		{admin, http.MethodPost, "/feeds/all", "", http.StatusMethodNotAllowed},
		{admin, http.MethodGet, "/feeds/all", "", http.StatusOK},
		{user, http.MethodGet, "/feeds/update", "", http.StatusForbidden},
		{admin, http.MethodPost, "/feeds/update", "", http.StatusMethodNotAllowed},
		{admin, http.MethodGet, "/feeds/update", "", http.StatusUnprocessableEntity},
		{user, http.MethodGet, "/folders", "", http.StatusOK},
		{user, http.MethodPost, "/folders", "{", http.StatusUnprocessableEntity},
		{user, http.MethodPatch, "/folders", "", http.StatusMethodNotAllowed},
		{user, http.MethodPut, "/folders/not-a-number", `{}`, http.StatusUnprocessableEntity},
		{user, http.MethodPut, "/folders/999999", `{"name":"Missing"}`, http.StatusNotFound},
		{user, http.MethodGet, "/folders/" + strconv.FormatInt(folder.ID, 10), "", http.StatusMethodNotAllowed},
		{user, http.MethodPost, "/folders/999999/read", `{}`, http.StatusOK},
		{user, http.MethodGet, "/folders/" + strconv.FormatInt(folder.ID, 10) + "/unknown", "", http.StatusNotFound},
		{user, http.MethodGet, "/feeds", "", http.StatusOK},
		{user, http.MethodPatch, "/feeds", "", http.StatusMethodNotAllowed},
		{user, http.MethodDelete, "/feeds/not-a-number", "", http.StatusUnprocessableEntity},
		{user, http.MethodDelete, "/feeds/999999", "", http.StatusNotFound},
		{user, http.MethodGet, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/move", "", http.StatusOK},
		{user, http.MethodPost, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/move", "{", http.StatusUnprocessableEntity},
		{user, http.MethodPost, "/feeds/999999/rename", `{"feedTitle":"Missing"}`, http.StatusNotFound},
		{user, http.MethodPost, "/feeds/" + strconv.FormatInt(feed.ID, 10) + "/unknown", `{}`, http.StatusNotFound},
		{user, http.MethodGet, "/items?type=x&getRead=x&batchSize=x&oldestFirst=x", "", http.StatusOK},
		{user, http.MethodGet, "/items/updated?lastModified=0", "", http.StatusOK},
		{user, http.MethodPost, "/items/read", `{}`, http.StatusOK},
		{user, http.MethodPost, "/items/read/multiple", "{", http.StatusUnprocessableEntity},
		{user, http.MethodPost, "/items/unknown/multiple", `{"items":[1]}`, http.StatusNotFound},
		{user, http.MethodPost, "/items/not-a-number/read", "", http.StatusUnprocessableEntity},
		{user, http.MethodGet, "/items/" + strconv.FormatInt(itemID, 10) + "/read", "", http.StatusNotFound},
		{user, http.MethodPost, "/items/" + strconv.FormatInt(itemID, 10) + "/unknown", "", http.StatusNotFound},
		{user, http.MethodGet, "/favicon/not-present", "", http.StatusNotFound},
		{user, http.MethodGet, "/missing", "", http.StatusNotFound},
	} {
		rec := call(tc.as, tc.method, tc.path, tc.body)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.path, rec.Code, tc.status, rec.Body.String())
		}
	}

	for _, action := range []string{"read", "unread", "star", "unstar"} {
		rec := call(user, http.MethodPost, "/items/"+strconv.FormatInt(itemID, 10)+"/"+action, "")
		if rec.Code != http.StatusOK {
			t.Errorf("item %s status=%d body=%s", action, rec.Code, rec.Body.String())
		}
	}
	for _, action := range []string{"read", "unread", "star", "starred", "unstar", "unstarred"} {
		rec := call(user, http.MethodPost, "/items/"+action+"/multiple", `{"items":[`+strconv.FormatInt(itemID, 10)+`]}`)
		if rec.Code != http.StatusOK {
			t.Errorf("multiple %s status=%d body=%s", action, rec.Code, rec.Body.String())
		}
	}

	rec := call(user, http.MethodPut, "/folders/"+strconv.FormatInt(folder.ID, 10), `{"name":"Renamed folder"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("rename folder status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = call(user, http.MethodPost, "/feeds/"+strconv.FormatInt(feed.ID, 10)+"/move", `{"folderId":0}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("move feed status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = call(user, http.MethodPost, "/feeds/"+strconv.FormatInt(feed.ID, 10)+"/rename", `{"feedTitle":"Renamed feed"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("rename feed status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = call(user, http.MethodPost, "/feeds/"+strconv.FormatInt(feed.ID, 10)+"/read", `{}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("read feed status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = call(user, http.MethodDelete, "/folders/"+strconv.FormatInt(folder.ID, 10), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete folder status=%d body=%s", rec.Code, rec.Body.String())
	}
}
