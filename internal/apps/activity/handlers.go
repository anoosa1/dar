package activity

import (
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type sourceOption struct {
	ID    string `json:"id"`
	Label string `json:"label"`
}

func (a *App) sources() []sourceOption {
	var out []sourceOption
	if a.registry != nil {
		for _, app := range a.registry.Apps() {
			if app.Name() == "activity" {
				continue
			}
			out = append(out, sourceOption{ID: app.Name(), Label: app.DisplayName()})
		}
	}
	out = append(out, sourceOption{ID: "system", Label: "System"})
	return out
}

func (a *App) sourceIDs() []string {
	options := a.sources()
	out := make([]string, 0, len(options))
	for _, option := range options {
		out = append(out, option.ID)
	}
	return out
}

func (a *App) validateSources(sources []string) ([]string, error) {
	valid := map[string]bool{}
	for _, source := range a.sourceIDs() {
		valid[source] = true
	}
	seen := map[string]bool{}
	out := make([]string, 0, len(sources))
	for _, source := range sources {
		source = strings.TrimSpace(source)
		if !valid[source] {
			return nil, fmt.Errorf("unknown activity source %q", source)
		}
		if !seen[source] {
			seen[source] = true
			out = append(out, source)
		}
	}
	if len(out) == 0 {
		return nil, errors.New("select at least one activity source")
	}
	return out, nil
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

func writeAPIError(w http.ResponseWriter, status int, err error) {
	writeJSON(w, status, map[string]string{"error": err.Error()})
}

func decodeStrict(w http.ResponseWriter, r *http.Request, dst any) error {
	r.Body = http.MaxBytesReader(w, r.Body, 64<<10)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(dst); err != nil {
		return err
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		if err == nil {
			return errors.New("request must contain one JSON value")
		}
		return err
	}
	return nil
}

func (a *App) handleList(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeAPIError(w, http.StatusUnauthorized, errors.New("unauthorized"))
		return
	}
	allowedQuery := map[string]bool{"source": true, "object_type": true, "object_id": true, "cursor": true, "limit": true}
	for key := range r.URL.Query() {
		if !allowedQuery[key] {
			writeAPIError(w, http.StatusBadRequest, fmt.Errorf("unknown query parameter %q", key))
			return
		}
	}
	allowed := map[string]bool{"": true, "all": true}
	for _, source := range a.sourceIDs() {
		allowed[source] = true
	}
	source := r.URL.Query().Get("source")
	if !allowed[source] {
		writeAPIError(w, http.StatusBadRequest, errors.New("unknown activity source"))
		return
	}
	limit := 50
	if raw := r.URL.Query().Get("limit"); raw != "" {
		n, err := strconv.Atoi(raw)
		if err != nil || n < 1 || n > 100 {
			writeAPIError(w, http.StatusBadRequest, errors.New("limit must be between 1 and 100"))
			return
		}
		limit = n
	}
	cur, err := decodeCursor(r.URL.Query().Get("cursor"))
	if err != nil {
		writeAPIError(w, http.StatusBadRequest, err)
		return
	}
	events, next, err := a.listEvents(r.Context(), listOptions{
		UserID: user.ID, Source: source, ObjectType: r.URL.Query().Get("object_type"),
		ObjectID: r.URL.Query().Get("object_id"), Cursor: cur, Limit: limit,
	})
	if err != nil {
		writeAPIError(w, http.StatusInternalServerError, errors.New("could not load activity"))
		return
	}
	if events == nil {
		events = []Event{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"events": events, "next_cursor": next})
}

func (a *App) settingsResponse(settings feedSettings, feedURL string) map[string]any {
	return map[string]any{
		"rss_enabled": settings.Enabled,
		"sources":     settings.Sources,
		"available":   a.sources(),
		"feed_url":    feedURL,
	}
}

func (a *App) handleSettings(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeAPIError(w, http.StatusUnauthorized, errors.New("unauthorized"))
		return
	}
	switch r.Method {
	case http.MethodGet:
		settings, found, err := a.loadFeedSettings(r.Context(), user.ID)
		if err != nil {
			writeAPIError(w, 500, errors.New("could not load activity settings"))
			return
		}
		if !found || len(settings.Sources) == 0 {
			settings.Sources = a.sourceIDs()
		}
		writeJSON(w, 200, a.settingsResponse(settings, ""))
	case http.MethodPut:
		var input struct {
			Enabled bool     `json:"rss_enabled"`
			Sources []string `json:"sources"`
		}
		if err := decodeStrict(w, r, &input); err != nil {
			writeAPIError(w, 400, err)
			return
		}
		sources, err := a.validateSources(input.Sources)
		if err != nil {
			writeAPIError(w, 400, err)
			return
		}
		current, _, err := a.loadFeedSettings(r.Context(), user.ID)
		if err != nil {
			writeAPIError(w, 500, errors.New("could not load activity settings"))
			return
		}
		current.Sources = sources
		feedURL := ""
		action := "updated"
		if input.Enabled {
			current.Enabled = true
			if current.TokenHash == "" {
				token, hash, tokenErr := newFeedToken()
				if tokenErr != nil {
					writeAPIError(w, 500, errors.New("could not create feed URL"))
					return
				}
				current.TokenHash = hash
				feedURL = a.baseURL + "/activity/rss/" + url.PathEscape(token)
				action = "enabled"
			}
		} else {
			if current.Enabled {
				action = "disabled"
			}
			current.Enabled = false
			current.TokenHash = ""
		}
		if err = a.saveFeedSettings(r.Context(), user.ID, current); err != nil {
			writeAPIError(w, 500, errors.New("could not save activity settings"))
			return
		}
		a.recordSystem(user.ID, user.DisplayName, "rss_"+action, "Activity RSS feed "+action)
		writeJSON(w, 200, a.settingsResponse(current, feedURL))
	default:
		w.Header().Set("Allow", "GET, PUT")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (a *App) handleRegenerate(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeAPIError(w, http.StatusUnauthorized, errors.New("unauthorized"))
		return
	}
	settings, found, err := a.loadFeedSettings(r.Context(), user.ID)
	if err != nil || !found || !settings.Enabled {
		writeAPIError(w, http.StatusConflict, errors.New("enable the RSS feed first"))
		return
	}
	token, hash, err := newFeedToken()
	if err != nil {
		writeAPIError(w, 500, errors.New("could not create feed URL"))
		return
	}
	settings.TokenHash = hash
	if err = a.saveFeedSettings(r.Context(), user.ID, settings); err != nil {
		writeAPIError(w, 500, errors.New("could not save feed URL"))
		return
	}
	a.recordSystem(user.ID, user.DisplayName, "rss_regenerated", "Activity RSS feed URL regenerated")
	writeJSON(w, 200, a.settingsResponse(settings, a.baseURL+"/activity/rss/"+url.PathEscape(token)))
}

func (a *App) recordSystem(userID, actorName, action, summary string) {
	if a.registry == nil {
		return
	}
	if strings.TrimSpace(actorName) == "" {
		actorName = "You"
	}
	if err := a.registry.EnqueueActivity(context.Background(), apps.ActivityEvent{
		ActorID: userID, ActorName: actorName, ActorKind: "user", Source: "system",
		Action: action, Summary: summary, Origin: "browser", RecipientIDs: []string{userID},
	}); err != nil {
		// The settings change has already succeeded; activity failures are
		// deliberately non-fatal.
		return
	}
}

type rss struct {
	XMLName xml.Name   `xml:"rss"`
	Version string     `xml:"version,attr"`
	Channel rssChannel `xml:"channel"`
}
type rssChannel struct {
	Title       string    `xml:"title"`
	Link        string    `xml:"link"`
	Description string    `xml:"description"`
	Items       []rssItem `xml:"item"`
}
type rssItem struct {
	Title       string `xml:"title"`
	Link        string `xml:"link,omitempty"`
	GUID        string `xml:"guid"`
	Description string `xml:"description"`
	PubDate     string `xml:"pubDate"`
	Category    string `xml:"category"`
}

func (a *App) handleRSS(w http.ResponseWriter, r *http.Request) {
	token := chi.URLParam(r, "token")
	if token == "" {
		http.NotFound(w, r)
		return
	}
	var userID, rawSources string
	err := a.withConn(r.Context(), func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`SELECT user_id,sources_json FROM activity_feed_settings WHERE enabled=1 AND token_hash=?`,
			&sqlitex.ExecOptions{Args: []any{tokenHash(token)}, ResultFunc: func(stmt *sqlite.Stmt) error {
				userID, rawSources = stmt.ColumnText(0), stmt.ColumnText(1)
				return nil
			}},
		)
	})
	if err != nil || userID == "" {
		http.NotFound(w, r)
		return
	}
	var sources []string
	if json.Unmarshal([]byte(rawSources), &sources) != nil || len(sources) == 0 {
		http.NotFound(w, r)
		return
	}
	events, _, err := a.listEvents(r.Context(), listOptions{UserID: userID, Sources: sources, Limit: 50})
	if err != nil {
		http.Error(w, "Activity feed is temporarily unavailable", http.StatusServiceUnavailable)
		return
	}
	channel := rssChannel{Title: "Dar Activity", Link: a.baseURL + "/activity", Description: "Selected activity from Dar"}
	for _, event := range events {
		link := ""
		if event.ObjectURL != "" {
			link = a.baseURL + event.ObjectURL
		}
		actor := event.ActorName
		if event.ActorKind == "system" {
			actor = "Dar"
		}
		channel.Items = append(channel.Items, rssItem{
			Title: event.Summary, Link: link, GUID: event.ID,
			Description: actor + " · " + event.Source + " · " + event.Origin,
			PubDate:     time.Unix(event.HappenedAt, 0).UTC().Format(time.RFC1123Z), Category: event.Source,
		})
	}
	data, err := xml.MarshalIndent(rss{Version: "2.0", Channel: channel}, "", "  ")
	if err != nil {
		http.Error(w, "Activity feed is temporarily unavailable", 500)
		return
	}
	w.Header().Set("Content-Type", "application/rss+xml; charset=utf-8")
	w.Header().Set("Cache-Control", "private, max-age=60")
	_, _ = w.Write([]byte(xml.Header))
	_, _ = w.Write(data)
}
