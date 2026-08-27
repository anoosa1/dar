package news

import (
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"html"
	"io"
	"log"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

const v13Base = "/index.php/apps/news/api/v1-3"

func cors(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Headers", "Authorization, Content-Type")
	w.Header().Set("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
	w.Header().Set("Access-Control-Max-Age", "600")
}

func apiError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, errNotFound):
		writeJSON(w, 404, map[string]string{"error": "not found"})
	case errors.Is(err, errInvalid):
		writeJSON(w, 422, map[string]string{"error": err.Error()})
	case errors.Is(err, errConflict) || strings.Contains(strings.ToLower(err.Error()), "unique"):
		writeJSON(w, 409, map[string]string{"error": "already exists"})
	default:
		log.Printf("news API internal error: %v", err)
		writeJSON(w, 500, map[string]string{"error": "internal error"})
	}
}

func (a *App) handleV13(w http.ResponseWriter, r *http.Request) {
	cors(w, r)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	user := server.GetUser(r)
	if user == nil {
		w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
		http.Error(w, "unauthorized", 401)
		return
	}
	path := strings.Trim(strings.TrimPrefix(r.URL.Path, v13Base), "/")
	parts := []string{}
	if path != "" {
		parts = strings.Split(path, "/")
	}
	uid := user.ID
	if path == "version" && r.Method == http.MethodGet {
		writeJSON(w, 200, map[string]string{"version": compatibilityVersion})
		return
	}
	if path == "status" && r.Method == http.MethodGet {
		writeJSON(w, 200, map[string]any{"version": compatibilityVersion, "warnings": map[string]bool{"improperlyConfiguredCron": false, "incorrectDbCharset": false}})
		return
	}
	if path == "user" && r.Method == http.MethodGet {
		writeJSON(w, 200, map[string]any{"userId": user.ID, "displayName": user.DisplayName, "lastLoginTimestamp": user.LastLoginAt.Unix(), "avatar": nil})
		return
	}
	if path == "cleanup/before-update" || path == "cleanup/after-update" {
		if !user.IsAdmin {
			http.Error(w, "forbidden", 403)
			return
		}
		if r.Method != http.MethodGet {
			http.Error(w, "method not allowed", 405)
			return
		}
		w.WriteHeader(200)
		return
	}
	if path == "feeds/all" {
		if !user.IsAdmin {
			http.Error(w, "forbidden", 403)
			return
		}
		a.adminAllFeeds(w, r)
		return
	}
	if path == "feeds/update" {
		if !user.IsAdmin {
			http.Error(w, "forbidden", 403)
			return
		}
		a.adminUpdateFeed(w, r)
		return
	}
	if len(parts) > 0 && parts[0] == "folders" {
		a.folderAPI(w, r, uid, parts)
		return
	}
	if len(parts) > 0 && parts[0] == "feeds" {
		a.feedAPI(w, r, uid, parts)
		return
	}
	if len(parts) > 0 && parts[0] == "items" {
		a.itemAPI(w, r, uid, parts)
		return
	}
	if len(parts) == 2 && parts[0] == "favicon" && r.Method == http.MethodGet {
		a.faviconAPI(w, r, uid, parts[1])
		return
	}
	http.NotFound(w, r)
}

func (a *App) folderAPI(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	ctx := r.Context()
	if len(p) == 1 {
		switch r.Method {
		case http.MethodGet:
			v, err := a.folders(ctx, uid)
			if err != nil {
				apiError(w, err)
				return
			}
			writeJSON(w, 200, map[string]any{"folders": v})
		case http.MethodPost:
			var body struct {
				Name string `json:"name"`
			}
			if readJSON(r, &body) != nil {
				apiError(w, errInvalid)
				return
			}
			f, err := a.createFolder(ctx, uid, body.Name)
			if err != nil {
				apiError(w, err)
				return
			}
			a.recordActivity(ctx, uid, "folder_created", "created news folder "+f.Name, "folder", strconv.FormatInt(f.ID, 10), f.Name, "api")
			writeJSON(w, 200, map[string]any{"folders": []Folder{f}})
		default:
			http.Error(w, "method not allowed", 405)
		}
		return
	}
	id, err := idPart(p[1])
	if err != nil {
		apiError(w, errInvalid)
		return
	}
	if len(p) == 2 {
		switch r.Method {
		case http.MethodPut:
			var body struct {
				Name string `json:"name"`
			}
			if readJSON(r, &body) != nil {
				apiError(w, errInvalid)
				return
			}
			err = a.renameFolder(ctx, uid, id, body.Name)
		case http.MethodDelete:
			err = a.deleteFolder(ctx, uid, id)
		default:
			http.Error(w, "method not allowed", 405)
			return
		}
		if err != nil {
			apiError(w, err)
			return
		}
		action := "folder_updated"
		summary := "updated news folder " + strconv.FormatInt(id, 10)
		if r.Method == http.MethodDelete {
			action, summary = "folder_deleted", "deleted news folder "+strconv.FormatInt(id, 10)
		}
		a.recordActivity(ctx, uid, action, summary, "folder", strconv.FormatInt(id, 10), strconv.FormatInt(id, 10), "api")
		w.WriteHeader(200)
		return
	}
	if len(p) == 3 && p[2] == "read" && r.Method == http.MethodPost {
		n := newestID(r)
		err = a.markScopeRead(ctx, uid, 0, id, n)
		if err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "folder_marked_read", "marked a news folder as read", "folder", strconv.FormatInt(id, 10), strconv.FormatInt(id, 10), "api")
		w.WriteHeader(200)
		return
	}
	http.NotFound(w, r)
}

type createFeedBody struct {
	URL               string `json:"url"`
	FolderID          int64  `json:"folderId"`
	AutoDiscover      bool   `json:"autoDiscover"`
	BasicAuthUser     string `json:"basicAuthUser"`
	BasicAuthPassword string `json:"basicAuthPassword"`
}

func (a *App) feedAPI(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	ctx := r.Context()
	if len(p) == 1 {
		switch r.Method {
		case http.MethodGet:
			v, err := a.feeds(ctx, uid)
			if err != nil {
				apiError(w, err)
				return
			}
			items, _ := a.items(ctx, uid, itemQuery{Type: 3, GetRead: true, BatchSize: 1})
			newest := int64(0)
			if len(items) > 0 {
				newest = items[0].ID
			}
			starred, _ := a.items(ctx, uid, itemQuery{Type: 2, GetRead: true, BatchSize: -1})
			out := map[string]any{"feeds": v, "starredCount": len(starred)}
			if newest > 0 {
				out["newestItemId"] = newest
			}
			writeJSON(w, 200, out)
		case http.MethodPost:
			var body createFeedBody
			if readJSON(r, &body) != nil {
				apiError(w, errInvalid)
				return
			}
			parsed, feedURL, err := a.resolveSubscription(ctx, body.URL, body.BasicAuthUser, body.BasicAuthPassword, body.AutoDiscover)
			if err != nil {
				apiError(w, fmt.Errorf("%w: %v", errInvalid, err))
				return
			}
			feed, err := a.insertFeed(ctx, uid, Feed{URL: feedURL, Title: parsed.Title, Link: parsed.Link, FolderID: body.FolderID, BasicAuthUser: body.BasicAuthUser, BasicAuthPassword: body.BasicAuthPassword})
			if err != nil {
				apiError(w, err)
				return
			}
			_ = a.updateFeed(ctx, uid, feed.ID)
			feed, _ = a.feed(ctx, uid, feed.ID)
			a.recordActivity(ctx, uid, "feed_created", "subscribed to "+feed.Title, "feed", strconv.FormatInt(feed.ID, 10), feed.Title, "api")
			writeJSON(w, 200, map[string]any{"feeds": []Feed{feed}})
		default:
			http.Error(w, "method not allowed", 405)
		}
		return
	}
	id, err := idPart(p[1])
	if err != nil {
		apiError(w, errInvalid)
		return
	}
	if len(p) == 2 && r.Method == http.MethodDelete {
		if err = a.deleteFeed(ctx, uid, id); err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "feed_deleted", "deleted news feed "+strconv.FormatInt(id, 10), "feed", strconv.FormatInt(id, 10), strconv.FormatInt(id, 10), "api")
		w.WriteHeader(200)
		return
	}
	if len(p) == 3 {
		switch p[2] {
		case "move":
			if r.Method != http.MethodPost {
				break
			}
			var body map[string]any
			if readJSON(r, &body) != nil {
				apiError(w, errInvalid)
				return
			}
			err = a.patchFeed(ctx, uid, id, map[string]any{"folderId": body["folderId"]})
		case "rename":
			if r.Method != http.MethodPost {
				break
			}
			var body struct {
				Title string `json:"feedTitle"`
			}
			if readJSON(r, &body) != nil {
				apiError(w, errInvalid)
				return
			}
			err = a.patchFeed(ctx, uid, id, map[string]any{"title": body.Title})
		case "read":
			if r.Method != http.MethodPost {
				break
			}
			err = a.markScopeRead(ctx, uid, id, 0, newestID(r))
		default:
			http.NotFound(w, r)
			return
		}
		if err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "feed_updated", "updated news feed "+strconv.FormatInt(id, 10), "feed", strconv.FormatInt(id, 10), strconv.FormatInt(id, 10), "api")
		w.WriteHeader(200)
		return
	}
	http.NotFound(w, r)
}

func newestID(r *http.Request) int64 {
	var body struct {
		Newest int64 `json:"newestItemId"`
	}
	_ = readJSON(r, &body)
	if body.Newest == 0 {
		body.Newest = 1 << 62
	}
	return body.Newest
}
func queryInt(r *http.Request, key string, def int64) int64 {
	v, err := strconv.ParseInt(r.URL.Query().Get(key), 10, 64)
	if err != nil {
		return def
	}
	return v
}
func queryBool(r *http.Request, key string, def bool) bool {
	v := r.URL.Query().Get(key)
	if v == "" {
		return def
	}
	b, err := strconv.ParseBool(v)
	if err != nil {
		return def
	}
	return b
}

func (a *App) itemAPI(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	ctx := r.Context()
	if len(p) == 1 && r.Method == http.MethodGet {
		q := itemQuery{Type: int(queryInt(r, "type", 3)), ID: queryInt(r, "id", 0), GetRead: queryBool(r, "getRead", false), BatchSize: int(queryInt(r, "batchSize", -1)), Offset: queryInt(r, "offset", 0), OldestFirst: queryBool(r, "oldestFirst", false)}
		v, err := a.items(ctx, uid, q)
		if err != nil {
			apiError(w, err)
			return
		}
		writeJSON(w, 200, map[string]any{"items": v})
		return
	}
	if len(p) == 2 && p[1] == "updated" && r.Method == http.MethodGet {
		q := itemQuery{Type: int(queryInt(r, "type", 3)), ID: queryInt(r, "id", 0), GetRead: true, BatchSize: -1, LastModified: queryInt(r, "lastModified", 0)}
		v, err := a.items(ctx, uid, q)
		if err != nil {
			apiError(w, err)
			return
		}
		writeJSON(w, 200, map[string]any{"items": v})
		return
	}
	if len(p) == 2 && p[1] == "read" && r.Method == http.MethodPost {
		if err := a.markScopeRead(ctx, uid, 0, 0, newestID(r)); err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "items_marked_read", "marked news items as read", "items", uid, "News items", "api")
		w.WriteHeader(200)
		return
	}
	if len(p) == 3 && p[2] == "multiple" && r.Method == http.MethodPost {
		var body struct {
			ItemIDs []int64 `json:"itemIds"`
			Items   []int64 `json:"items"`
		}
		if readJSON(r, &body) != nil {
			apiError(w, errInvalid)
			return
		}
		if len(body.ItemIDs) == 0 {
			body.ItemIDs = body.Items
		}
		column := ""
		value := false
		switch p[1] {
		case "read":
			column = "unread"
		case "unread":
			column = "unread"
			value = true
		case "star", "starred":
			column = "starred"
			value = true
		case "unstar", "unstarred":
			column = "starred"
		}
		if column == "" {
			http.NotFound(w, r)
			return
		}
		if err := a.setItemState(ctx, uid, body.ItemIDs, column, value); err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "items_updated", fmt.Sprintf("updated %d news items", len(body.ItemIDs)), "items", uid, "News items", "api")
		w.WriteHeader(200)
		return
	}
	if len(p) == 3 {
		itemID, err := idPart(p[1])
		if err != nil {
			apiError(w, errInvalid)
			return
		}
		column := ""
		value := false
		switch p[2] {
		case "read":
			column = "unread"
		case "unread":
			column = "unread"
			value = true
		case "star":
			column = "starred"
			value = true
		case "unstar":
			column = "starred"
		}
		if column == "" || r.Method != http.MethodPost {
			http.NotFound(w, r)
			return
		}
		if err := a.setItemState(ctx, uid, []int64{itemID}, column, value); err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(ctx, uid, "item_updated", "updated a news item", "item", strconv.FormatInt(itemID, 10), strconv.FormatInt(itemID, 10), "api")
		w.WriteHeader(200)
		return
	}
	http.NotFound(w, r)
}

func (a *App) adminAllFeeds(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", 405)
		return
	}
	var feeds []map[string]any
	err := a.withConn(r.Context(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,user_id FROM news_feeds WHERE update_enabled=1 ORDER BY id`, &sqlitex.ExecOptions{ResultFunc: func(s *sqlite.Stmt) error {
			feeds = append(feeds, map[string]any{"id": s.ColumnInt64(0), "userId": s.ColumnText(1)})
			return nil
		}})
	})
	if err != nil {
		apiError(w, err)
		return
	}
	writeJSON(w, 200, map[string]any{"feeds": feeds})
}
func (a *App) adminUpdateFeed(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := r.URL.Query().Get("userId")
	id := queryInt(r, "feedId", 0)
	if uid == "" || id == 0 {
		apiError(w, errInvalid)
		return
	}
	if err := a.updateFeed(r.Context(), uid, id); err != nil {
		apiError(w, err)
		return
	}
	w.WriteHeader(200)
}

func (a *App) faviconAPI(w http.ResponseWriter, r *http.Request, uid, hash string) {
	feeds, err := a.feeds(r.Context(), uid)
	if err != nil {
		apiError(w, err)
		return
	}
	for _, f := range feeds {
		if hashMD5(f.URL) != hash {
			continue
		}
		if f.FaviconLink == "" {
			http.NotFound(w, r)
			return
		}
		data, _, header, err := a.fetchBytes(r.Context(), f.FaviconLink, faviconBodyLimit, "", "")
		if err != nil {
			apiError(w, err)
			return
		}
		typ := header.Get("Content-Type")
		if !strings.HasPrefix(typ, "image/") {
			typ = "image/x-icon"
		}
		w.Header().Set("Content-Type", typ)
		w.Header().Set("Cache-Control", "public, max-age=86400")
		_, _ = w.Write(data)
		return
	}
	http.NotFound(w, r)
}

type opml struct {
	XMLName xml.Name `xml:"opml"`
	Version string   `xml:"version,attr,omitempty"`
	Body    opmlBody `xml:"body"`
}
type opmlBody struct {
	Outlines []outline `xml:"outline"`
}
type outline struct {
	Text     string    `xml:"text,attr"`
	Title    string    `xml:"title,attr,omitempty"`
	Type     string    `xml:"type,attr,omitempty"`
	XMLURL   string    `xml:"xmlUrl,attr,omitempty"`
	HTMLURL  string    `xml:"htmlUrl,attr,omitempty"`
	Children []outline `xml:"outline"`
}

func (a *App) handleAux(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	path := strings.Trim(strings.TrimPrefix(r.URL.Path, "/news/api/"), "/")
	if strings.HasPrefix(path, "content/") && r.Method == http.MethodGet {
		a.articleContent(w, r, uid, strings.TrimPrefix(path, "content/"))
		return
	}
	switch path {
	case "preferences":
		if r.Method == http.MethodGet {
			p, err := a.preferences(r.Context(), uid)
			if err != nil {
				apiError(w, err)
				return
			}
			writeJSON(w, 200, p)
			return
		}
		if r.Method == http.MethodPut {
			var p Preferences
			if readJSON(r, &p) != nil {
				apiError(w, errInvalid)
				return
			}
			if err := a.savePreferences(r.Context(), uid, p); err != nil {
				apiError(w, err)
				return
			}
			a.recordActivity(r.Context(), uid, "preferences_updated", "updated News preferences", "preferences", uid, "News preferences", "api")
			writeJSON(w, 200, p)
			return
		}
	case "refresh":
		if r.Method == http.MethodPost {
			a.refreshUserAsync(uid)
			writeJSON(w, 202, map[string]string{"status": "refreshing"})
			return
		}
		if r.Method == http.MethodGet {
			a.mu.Lock()
			state := a.refresh[uid]
			a.mu.Unlock()
			writeJSON(w, 200, state)
			return
		}
	case "recent":
		if r.Method == http.MethodGet {
			items, err := a.items(r.Context(), uid, itemQuery{Type: 3, GetRead: true, BatchSize: -1, Recent: true})
			if err != nil {
				apiError(w, err)
				return
			}
			writeJSON(w, 200, map[string]any{"items": items})
			return
		}
	case "explore":
		if r.Method == http.MethodGet {
			a.explore(w, r)
			return
		}
	case "opml":
		a.opmlAPI(w, r, uid)
		return
	case "articles":
		a.articleArchiveAPI(w, r, uid)
		return
	}
	if strings.HasPrefix(path, "feed/") && r.Method == http.MethodPatch {
		id, err := idPart(strings.TrimPrefix(path, "feed/"))
		if err != nil {
			apiError(w, errInvalid)
			return
		}
		var fields map[string]any
		if readJSON(r, &fields) != nil {
			apiError(w, errInvalid)
			return
		}
		if err := a.patchFeed(r.Context(), uid, id, fields); err != nil {
			apiError(w, err)
			return
		}
		feed, err := a.feed(r.Context(), uid, id)
		if err != nil {
			apiError(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "feed_updated", "updated news feed "+feed.Title, "feed", strconv.FormatInt(id, 10), feed.Title, "api")
		writeJSON(w, 200, feed)
		return
	}
	if strings.HasPrefix(path, "items/") && strings.HasSuffix(path, "/view") && r.Method == http.MethodPost {
		idText := strings.TrimSuffix(strings.TrimPrefix(path, "items/"), "/view")
		id, err := idPart(idText)
		if err != nil {
			apiError(w, errInvalid)
			return
		}
		if err := a.viewItem(r.Context(), uid, id); err != nil {
			apiError(w, err)
			return
		}
		w.WriteHeader(200)
		return
	}
	http.NotFound(w, r)
}

func (a *App) articleContent(w http.ResponseWriter, r *http.Request, uid, rawID string) {
	id, err := idPart(rawID)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	var body, enclosureMime, enclosureLink, itemURL, itemTitle string
	found := false
	err = a.withConn(r.Context(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT body,COALESCE(enclosure_mime,''),COALESCE(enclosure_link,''),url,title FROM news_items WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error {
			body = s.ColumnText(0)
			enclosureMime = s.ColumnText(1)
			enclosureLink = s.ColumnText(2)
			itemURL = s.ColumnText(3)
			itemTitle = s.ColumnText(4)
			found = true
			return nil
		}})
	})
	if err != nil || !found {
		http.NotFound(w, r)
		return
	}
	images := "'none'"
	if r.URL.Query().Get("images") == "1" {
		images = "https: http: data:"
	}
	youtubeFallback := youtubeFallbackHTML(itemURL, itemTitle)
	if youtubeFallback != "" && images == "'none'" {
		images = "https://i.ytimg.com"
	}
	media := "'none'"
	enclosure := ""
	if r.URL.Query().Get("enclosures") == "1" {
		enclosure = enclosureHTML(enclosureMime, enclosureLink)
		if enclosure != "" {
			media = "https: http:"
		}
	}
	if body == "" && enclosure == "" && youtubeFallback == "" {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "private, no-store")
	w.Header().Set("X-Frame-Options", "SAMEORIGIN")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src "+images+"; media-src "+media+"; frame-src 'none'; connect-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'")
	palette := theme.MochaPink()
	_, _ = io.WriteString(w, `<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta name="theme-color" content="`+palette.Base+`"><meta name="color-scheme" content="dark"><style>`+palette.StandaloneCSS()+`main{all:unset}img,video{max-width:100%;height:auto}video,audio{display:block;width:100%;margin:0 0 1rem}pre{white-space:pre-wrap}.youtube-fallback{width:min(52rem,calc(100% - 1rem));margin:6vh auto;text-align:center}.youtube-fallback img{display:block;width:100%;aspect-ratio:16/9;object-fit:cover;border:1px solid `+palette.OutlineVariant+`;border-radius:1rem}.youtube-fallback p{margin:.9rem 0 0;color:`+palette.OnSurfaceVariant+`}</style></head><body>`+enclosure+youtubeFallback+body+`</body></html>`)
}

func youtubeFallbackHTML(rawURL, title string) string {
	videoID := youtubeVideoID(rawURL)
	if videoID == "" {
		return ""
	}
	alt := "YouTube video thumbnail"
	if title = strings.TrimSpace(title); title != "" {
		alt = "Preview for " + title
	}
	return `<section class="youtube-fallback"><img src="https://i.ytimg.com/vi/` + videoID + `/hqdefault.jpg" alt="` + html.EscapeString(alt) + `"><p>This RSS entry is a YouTube video. Use <strong>Watch on YouTube</strong> above to view it.</p></section>`
}

func youtubeVideoID(rawURL string) string {
	u, err := url.Parse(strings.TrimSpace(rawURL))
	if err != nil || (u.Scheme != "https" && u.Scheme != "http") {
		return ""
	}
	host := strings.ToLower(u.Hostname())
	var id string
	switch host {
	case "youtube.com", "www.youtube.com", "m.youtube.com":
		switch {
		case u.Path == "/watch":
			id = u.Query().Get("v")
		case strings.HasPrefix(u.Path, "/shorts/"):
			id = strings.TrimPrefix(u.Path, "/shorts/")
		case strings.HasPrefix(u.Path, "/live/"):
			id = strings.TrimPrefix(u.Path, "/live/")
		}
	case "youtu.be", "www.youtu.be":
		id = strings.TrimPrefix(u.Path, "/")
	}
	if strings.Contains(id, "/") || len(id) != 11 {
		return ""
	}
	for _, r := range id {
		if (r < 'a' || r > 'z') && (r < 'A' || r > 'Z') && (r < '0' || r > '9') && r != '-' && r != '_' {
			return ""
		}
	}
	return id
}

func enclosureHTML(mimeType, rawURL string) string {
	u, err := url.Parse(strings.TrimSpace(rawURL))
	if err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Host == "" {
		return ""
	}
	src := html.EscapeString(u.String())
	switch {
	case strings.HasPrefix(strings.ToLower(strings.TrimSpace(mimeType)), "video/"):
		return `<video controls playsinline preload="metadata" src="` + src + `">Video playback is unavailable. <a href="` + src + `" target="_blank" rel="noopener noreferrer">Open the video</a>.</video>`
	case strings.HasPrefix(strings.ToLower(strings.TrimSpace(mimeType)), "audio/"):
		return `<audio controls preload="metadata" src="` + src + `">Audio playback is unavailable. <a href="` + src + `" target="_blank" rel="noopener noreferrer">Open the audio</a>.</audio>`
	default:
		return ""
	}
}

func (a *App) opmlAPI(w http.ResponseWriter, r *http.Request, uid string) {
	if r.Method == http.MethodGet {
		folders, _ := a.folders(r.Context(), uid)
		feeds, _ := a.feeds(r.Context(), uid)
		root := opml{Version: "2.0"}
		byFolder := map[int64][]outline{}
		for _, f := range feeds {
			byFolder[f.FolderID] = append(byFolder[f.FolderID], outline{Text: f.Title, Title: f.Title, Type: "rss", XMLURL: f.URL, HTMLURL: f.Link})
		}
		root.Body.Outlines = append(root.Body.Outlines, byFolder[0]...)
		for _, folder := range folders {
			root.Body.Outlines = append(root.Body.Outlines, outline{Text: folder.Name, Title: folder.Name, Children: byFolder[folder.ID]})
		}
		w.Header().Set("Content-Type", "text/x-opml; charset=utf-8")
		w.Header().Set("Content-Disposition", `attachment; filename="dar-news.opml"`)
		_, _ = w.Write([]byte(xml.Header))
		_ = xml.NewEncoder(w).Encode(root)
		return
	}
	if r.Method == http.MethodPost {
		data, err := io.ReadAll(io.LimitReader(r.Body, 10<<20))
		if err != nil {
			apiError(w, err)
			return
		}
		var root opml
		if xml.Unmarshal(data, &root) != nil {
			apiError(w, errInvalid)
			return
		}
		created := 0
		var walk func([]outline, int64)
		walk = func(nodes []outline, folderID int64) {
			for _, node := range nodes {
				next := folderID
				if node.XMLURL == "" && len(node.Children) > 0 {
					if f, err := a.createFolder(r.Context(), uid, fallback(node.Title, node.Text)); err == nil {
						next = f.ID
					}
					walk(node.Children, next)
					continue
				}
				if node.XMLURL != "" {
					parsed, feedURL, err := a.resolveSubscription(r.Context(), node.XMLURL, "", "", true)
					if err != nil {
						continue
					}
					if _, err = a.insertFeed(r.Context(), uid, Feed{URL: feedURL, Title: fallback(node.Title, parsed.Title), Link: parsed.Link, FolderID: next}); err == nil {
						created++
					}
				}
			}
		}
		walk(root.Body.Outlines, 0)
		writeJSON(w, 200, map[string]int{"created": created})
		return
	}
	http.Error(w, "method not allowed", 405)
}

func (a *App) articleArchiveAPI(w http.ResponseWriter, r *http.Request, uid string) {
	if r.Method == http.MethodPost {
		defer a.markSearchDirty()
	}
	if r.Method == http.MethodGet {
		items, err := a.items(r.Context(), uid, itemQuery{Type: 3, GetRead: true, BatchSize: -1})
		if err != nil {
			apiError(w, err)
			return
		}
		w.Header().Set("Content-Disposition", `attachment; filename="dar-news-articles.json"`)
		writeJSON(w, 200, map[string]any{"items": items})
		return
	}
	if r.Method == http.MethodPost {
		data, err := readLimitedBody(r.Body, 50<<20)
		if err != nil {
			apiError(w, fmt.Errorf("%w: %v", errInvalid, err))
			return
		}
		archive, err := decodeArticleArchive(data)
		if err != nil {
			apiError(w, fmt.Errorf("%w: invalid article archive: %v", errInvalid, err))
			return
		}
		imported, skipped, duplicates := 0, 0, 0
		err = a.withConn(r.Context(), func(c *sqlite.Conn) (err error) {
			end, err := sqlitex.ImmediateTransaction(c)
			if err != nil {
				return err
			}
			defer end(&err)
			feedIDs := map[string]int64{}
			if err := sqlitex.Execute(c, `SELECT id,url,link FROM news_feeds WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
				id := s.ColumnInt64(0)
				for _, raw := range []string{s.ColumnText(1), s.ColumnText(2)} {
					if key := canonicalFeedLink(raw); key != "" {
						feedIDs[key] = id
					}
				}
				return nil
			}}); err != nil {
				return err
			}
			for _, item := range archive.Items {
				feedID := item.FeedID
				if feedID == 0 {
					feedID = feedIDs[canonicalFeedLink(item.FeedLink)]
				}
				if feedID == 0 {
					skipped++
					continue
				}
				guid := strings.TrimSpace(item.GUID)
				if guid == "" {
					guid = strings.TrimSpace(item.URL)
				}
				if guid == "" {
					skipped++
					continue
				}
				guidHash := item.GUIDHash
				if guidHash == "" {
					guidHash = hashMD5(guid)
				}
				fingerprint := item.Fingerprint
				if fingerprint == "" {
					fingerprint = hashSHA(item.Title + "\x00" + item.Body + "\x00" + item.URL)
				}
				if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,url,title,author,pub_date,body,enclosure_mime,enclosure_link,media_thumbnail,media_description,unread,starred,rtl,last_modified,fingerprint,viewed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id,feed_id,guid_hash) DO NOTHING`, &sqlitex.ExecOptions{Args: []any{uid, feedID, guid, guidHash, item.URL, item.Title, ptrValue(item.Author), item.PubDate, item.Body, ptrValue(item.EnclosureMime), ptrValue(item.EnclosureLink), ptrValue(item.MediaThumbnail), ptrValue(item.MediaDescription), boolInt(item.Unread), boolInt(item.Starred), boolInt(item.RTL), time.Now().Unix(), fingerprint, item.ViewedAt}}); err != nil {
					return err
				}
				if c.Changes() == 0 {
					duplicates++
				} else {
					imported++
				}
			}
			return nil
		})
		if err != nil {
			apiError(w, err)
			return
		}
		writeJSON(w, 200, map[string]int{"imported": imported, "skipped": skipped, "duplicates": duplicates})
		return
	}
	http.Error(w, "method not allowed", 405)
}

type importedArticle struct {
	Item
	FeedLink string `json:"feedLink"`
}

type articleArchive struct {
	Items []importedArticle `json:"items"`
}

func decodeArticleArchive(data []byte) (articleArchive, error) {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "" {
		return articleArchive{}, fmt.Errorf("file is empty")
	}
	var archive articleArchive
	if strings.HasPrefix(trimmed, "[") {
		if err := json.Unmarshal(data, &archive.Items); err != nil {
			return articleArchive{}, err
		}
	} else {
		if err := json.Unmarshal(data, &archive); err != nil {
			return articleArchive{}, err
		}
	}
	if archive.Items == nil {
		return articleArchive{}, fmt.Errorf("missing articles array")
	}
	return archive, nil
}

func readLimitedBody(r io.Reader, limit int64) ([]byte, error) {
	data, err := io.ReadAll(io.LimitReader(r, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("file exceeds %d MiB", limit>>20)
	}
	return data, nil
}

func canonicalFeedLink(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ""
	}
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return strings.TrimRight(raw, "/")
	}
	u.Scheme = strings.ToLower(u.Scheme)
	u.Host = strings.ToLower(u.Host)
	u.Fragment = ""
	return strings.TrimRight(u.String(), "/")
}

func ptrValue(v *string) any {
	if v == nil {
		return nil
	}
	return *v
}

var defaultExplore = []map[string]string{{"title": "Nextcloud News", "description": "News from the Nextcloud project", "url": "https://nextcloud.com/blog/feed/"}, {"title": "Wikinews", "description": "Collaborative journalism from Wikimedia", "url": "https://en.wikinews.org/w/index.php?title=Special:NewsFeed&feed=rss"}, {"title": "NASA Breaking News", "description": "News from NASA", "url": "https://www.nasa.gov/news-release/feed/"}, {"title": "Public Domain Review", "description": "Curious works from art and literary history", "url": "https://publicdomainreview.org/rss.xml"}}

func (a *App) explore(w http.ResponseWriter, r *http.Request) {
	if a.exploreURL == "" {
		writeJSON(w, 200, map[string]any{"feeds": defaultExplore})
		return
	}
	data, _, _, err := a.fetchBytes(r.Context(), a.exploreURL, 1<<20, "", "")
	if err != nil {
		apiError(w, err)
		return
	}
	var feeds any
	if json.Unmarshal(data, &feeds) != nil {
		apiError(w, errInvalid)
		return
	}
	writeJSON(w, 200, map[string]any{"feeds": feeds})
}
