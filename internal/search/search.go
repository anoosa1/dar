// Package search provides Dar's user-scoped cross-app full-text index.
package search

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"html"
	"log"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

const (
	defaultInterval   = 30 * time.Minute
	suggestLimit      = 3
	pageLimit         = 20
	recentLimit       = 8
	indexBatchSize    = 32
	cleanupBatchSize  = 32
	cleanupYieldDelay = 2 * time.Millisecond
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS search_documents (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		user_id TEXT NOT NULL,
		provider TEXT NOT NULL,
		object_id TEXT NOT NULL,
		chunk_id TEXT NOT NULL,
		title TEXT NOT NULL,
		subtitle TEXT NOT NULL DEFAULT '',
		keywords TEXT NOT NULL DEFAULT '',
		body TEXT NOT NULL DEFAULT '',
		url TEXT NOT NULL,
		anchor TEXT NOT NULL DEFAULT '',
		modified_at INTEGER NOT NULL DEFAULT 0,
		UNIQUE(user_id,provider,object_id,chunk_id)
	);
	CREATE INDEX IF NOT EXISTS idx_search_documents_parent ON search_documents(user_id,provider,object_id);
	CREATE INDEX IF NOT EXISTS idx_search_documents_recent ON search_documents(user_id,modified_at DESC);`,
	`CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
		title, keywords, body,
		content='search_documents',
		content_rowid='id',
		tokenize='unicode61 remove_diacritics 2'
	);
	CREATE TRIGGER IF NOT EXISTS search_documents_ai AFTER INSERT ON search_documents BEGIN
		INSERT INTO search_fts(rowid,title,keywords,body) VALUES(new.id,new.title,new.keywords,new.body);
	END;
	CREATE TRIGGER IF NOT EXISTS search_documents_ad AFTER DELETE ON search_documents BEGIN
		INSERT INTO search_fts(search_fts,rowid,title,keywords,body) VALUES('delete',old.id,old.title,old.keywords,old.body);
	END;
	CREATE TRIGGER IF NOT EXISTS search_documents_au AFTER UPDATE ON search_documents BEGIN
		INSERT INTO search_fts(search_fts,rowid,title,keywords,body) VALUES('delete',old.id,old.title,old.keywords,old.body);
		INSERT INTO search_fts(rowid,title,keywords,body) VALUES(new.id,new.title,new.keywords,new.body);
	END;`,
	`CREATE TABLE IF NOT EXISTS search_staging (
		run_id TEXT NOT NULL,
		user_id TEXT NOT NULL,
		provider TEXT NOT NULL,
		object_id TEXT NOT NULL,
		chunk_id TEXT NOT NULL,
		title TEXT NOT NULL,
		subtitle TEXT NOT NULL DEFAULT '',
		keywords TEXT NOT NULL DEFAULT '',
		body TEXT NOT NULL DEFAULT '',
		url TEXT NOT NULL,
		anchor TEXT NOT NULL DEFAULT '',
		modified_at INTEGER NOT NULL DEFAULT 0,
		PRIMARY KEY(run_id,object_id,chunk_id)
	);`,
	`CREATE TABLE IF NOT EXISTS search_provider_runs (
		user_id TEXT NOT NULL,
		provider TEXT NOT NULL,
		last_success_at INTEGER NOT NULL DEFAULT 0,
		last_error_at INTEGER NOT NULL DEFAULT 0,
		error_text TEXT NOT NULL DEFAULT '',
		PRIMARY KEY(user_id,provider)
	);`,
	`DROP TRIGGER IF EXISTS search_documents_ai;
	DROP TRIGGER IF EXISTS search_documents_ad;
	DROP TRIGGER IF EXISTS search_documents_au;
	DROP TABLE IF EXISTS search_fts;
	DROP TABLE IF EXISTS search_documents;
	DROP TABLE IF EXISTS search_staging;
	DELETE FROM search_provider_runs;

	CREATE TABLE search_documents (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		user_id TEXT NOT NULL,
		provider TEXT NOT NULL,
		generation TEXT NOT NULL,
		object_id TEXT NOT NULL,
		chunk_id TEXT NOT NULL,
		title TEXT NOT NULL,
		subtitle TEXT NOT NULL DEFAULT '',
		keywords TEXT NOT NULL DEFAULT '',
		body TEXT NOT NULL DEFAULT '',
		url TEXT NOT NULL,
		anchor TEXT NOT NULL DEFAULT '',
		modified_at INTEGER NOT NULL DEFAULT 0,
		UNIQUE(user_id,provider,generation,object_id,chunk_id)
	);
	CREATE INDEX idx_search_documents_parent ON search_documents(user_id,provider,generation,object_id);
	CREATE INDEX idx_search_documents_recent ON search_documents(user_id,modified_at DESC);

	CREATE VIRTUAL TABLE search_fts USING fts5(
		title, keywords, body,
		content='search_documents',
		content_rowid='id',
		tokenize='unicode61 remove_diacritics 2'
	);
	CREATE TRIGGER search_documents_ai AFTER INSERT ON search_documents BEGIN
		INSERT INTO search_fts(rowid,title,keywords,body) VALUES(new.id,new.title,new.keywords,new.body);
	END;
	CREATE TRIGGER search_documents_ad AFTER DELETE ON search_documents BEGIN
		INSERT INTO search_fts(search_fts,rowid,title,keywords,body) VALUES('delete',old.id,old.title,old.keywords,old.body);
	END;
	CREATE TRIGGER search_documents_au AFTER UPDATE ON search_documents BEGIN
		INSERT INTO search_fts(search_fts,rowid,title,keywords,body) VALUES('delete',old.id,old.title,old.keywords,old.body);
		INSERT INTO search_fts(rowid,title,keywords,body) VALUES(new.id,new.title,new.keywords,new.body);
	END;

	CREATE TABLE search_provider_generations (
		user_id TEXT NOT NULL,
		provider TEXT NOT NULL,
		active_generation TEXT NOT NULL,
		PRIMARY KEY(user_id,provider)
	);`,
}

func Migrations() []string { return append([]string(nil), migrations...) }

type Provider struct {
	Name        string `json:"name"`
	DisplayName string `json:"display_name"`
	Icon        string `json:"icon"`
	Enabled     bool   `json:"enabled"`
}

type Preferences struct {
	Providers []Provider `json:"providers"`
}

type Match struct {
	ChunkID string `json:"chunk_id"`
	Snippet string `json:"snippet"`
	Anchor  string `json:"anchor,omitempty"`
}

type Result struct {
	Provider   string `json:"provider"`
	ObjectID   string `json:"object_id"`
	Title      string `json:"title"`
	Subtitle   string `json:"subtitle,omitempty"`
	URL        string `json:"url"`
	ModifiedAt int64  `json:"modified_at"`
	Match      Match  `json:"match"`
	MatchCount int    `json:"match_count"`
}

type Group struct {
	Provider    string   `json:"provider"`
	DisplayName string   `json:"display_name"`
	Icon        string   `json:"icon"`
	Results     []Result `json:"results"`
	NextCursor  string   `json:"next_cursor,omitempty"`
}

type QueryResponse struct {
	Recent         []Result `json:"recent"`
	Groups         []Group  `json:"groups"`
	StaleProviders []string `json:"stale_providers,omitempty"`
}

type Service struct {
	db       *db.DB
	registry *apps.Registry
	interval time.Duration

	cancel context.CancelFunc
	wg     sync.WaitGroup
	dirty  chan string
}

func New(database *db.DB, registry *apps.Registry, interval time.Duration) *Service {
	if interval <= 0 {
		interval = defaultInterval
	}
	return &Service{db: database, registry: registry, interval: interval, dirty: make(chan string, 64)}
}

func (s *Service) Writer(provider string) apps.SearchIndexWriter {
	return &writer{service: s, provider: provider}
}

func (s *Service) Start(ctx context.Context) error {
	if s.db == nil || s.registry == nil {
		return errors.New("search database and registry are required")
	}
	workerCtx, cancel := context.WithCancel(ctx)
	s.cancel = cancel
	s.wg.Add(1)
	go s.run(workerCtx)
	return nil
}

func (s *Service) Shutdown(ctx context.Context) error {
	if s.cancel != nil {
		s.cancel()
	}
	done := make(chan struct{})
	go func() { s.wg.Wait(); close(done) }()
	select {
	case <-done:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (s *Service) run(ctx context.Context) {
	defer s.wg.Done()
	if err := s.cleanupInactiveGenerations(ctx); err != nil && ctx.Err() == nil {
		log.Printf("search: clean inactive generations: %v", err)
	}
	s.reconcileAll(ctx, "")
	ticker := time.NewTicker(s.interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case provider := <-s.dirty:
			s.reconcileAll(ctx, provider)
		case <-ticker.C:
			s.reconcileAll(ctx, "")
		}
	}
}

func (s *Service) reconcileAll(ctx context.Context, onlyProvider string) {
	users, err := s.userIDs(ctx)
	if err != nil {
		log.Printf("search: list users: %v", err)
		return
	}
	for _, userID := range users {
		for _, app := range s.registry.Apps() {
			if ctx.Err() != nil {
				return
			}
			if onlyProvider != "" && app.Name() != onlyProvider {
				continue
			}
			indexer, ok := app.(apps.SearchIndexer)
			if !ok {
				continue
			}
			if err := s.reconcileProvider(ctx, userID, app.Name(), indexer); err != nil {
				log.Printf("search: index %s for user %s: %v", app.Name(), userID, err)
			}
		}
	}
}

func (s *Service) userIDs(ctx context.Context) ([]string, error) {
	var out []string
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT id FROM users ORDER BY id`, &sqlitex.ExecOptions{ResultFunc: func(stmt *sqlite.Stmt) error {
			out = append(out, stmt.ColumnText(0))
			return nil
		}})
	})
	return out, err
}

type generationSink struct {
	service                      *Service
	generation, userID, provider string
	pending                      []apps.SearchDocument
}

func (s *generationSink) AddSearchDocument(ctx context.Context, doc apps.SearchDocument) error {
	if err := validateDocument(doc); err != nil {
		return err
	}
	s.pending = append(s.pending, doc)
	if len(s.pending) < indexBatchSize {
		return nil
	}
	return s.flush(ctx)
}

func (s *generationSink) flush(ctx context.Context) error {
	if len(s.pending) == 0 {
		return nil
	}
	err := s.service.withConn(ctx, func(conn *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(conn)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		for _, doc := range s.pending {
			if txErr = insertDocument(conn, s.userID, s.provider, s.generation, doc); txErr != nil {
				return txErr
			}
		}
		return nil
	})
	if err == nil {
		s.pending = s.pending[:0]
	}
	return err
}

func (s *Service) reconcileProvider(ctx context.Context, userID, provider string, indexer apps.SearchIndexer) error {
	runID := randomID()
	sink := &generationSink{service: s, generation: runID, userID: userID, provider: provider}
	if err := indexer.IndexSearch(ctx, userID, sink); err != nil {
		_ = s.finishFailedRun(context.Background(), runID, userID, provider, err)
		return err
	}
	if err := sink.flush(ctx); err != nil {
		_ = s.finishFailedRun(context.Background(), runID, userID, provider, err)
		return err
	}
	err := s.withConn(ctx, func(conn *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(conn)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		if txErr = sqlitex.Execute(conn, `INSERT INTO search_provider_generations(user_id,provider,active_generation)
			VALUES(?,?,?)
			ON CONFLICT(user_id,provider) DO UPDATE SET active_generation=excluded.active_generation`,
			&sqlitex.ExecOptions{Args: []any{userID, provider, runID}}); txErr != nil {
			return txErr
		}
		txErr = sqlitex.Execute(conn, `INSERT INTO search_provider_runs(user_id,provider,last_success_at,last_error_at,error_text)
			VALUES(?,?,?,0,'')
			ON CONFLICT(user_id,provider) DO UPDATE SET last_success_at=excluded.last_success_at,last_error_at=0,error_text=''`,
			&sqlitex.ExecOptions{Args: []any{userID, provider, time.Now().Unix()}})
		return txErr
	})
	if err != nil {
		_ = s.finishFailedRun(context.Background(), runID, userID, provider, err)
		return err
	}
	if err := s.cleanupProviderGenerations(ctx, userID, provider, runID); err != nil && ctx.Err() == nil {
		log.Printf("search: clean old %s generations for user %s: %v", provider, userID, err)
	}
	return nil
}

func (s *Service) finishFailedRun(ctx context.Context, runID, userID, provider string, cause error) error {
	markErr := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `INSERT INTO search_provider_runs(user_id,provider,last_error_at,error_text)
			VALUES(?,?,?,?)
			ON CONFLICT(user_id,provider) DO UPDATE SET
				last_error_at=max(excluded.last_error_at,search_provider_runs.last_success_at+1),
				error_text=excluded.error_text`,
			&sqlitex.ExecOptions{Args: []any{userID, provider, time.Now().Unix(), truncate(cause.Error(), 500)}})
	})
	cleanupErr := s.cleanupGeneration(ctx, userID, provider, runID)
	return errors.Join(markErr, cleanupErr)
}

func (s *Service) cleanupGeneration(ctx context.Context, userID, provider, generation string) error {
	return s.cleanupBatches(ctx, `DELETE FROM search_documents WHERE id IN (
		SELECT id FROM search_documents
		WHERE user_id=? AND provider=? AND generation=?
		ORDER BY id LIMIT ?
	)`, []any{userID, provider, generation})
}

func (s *Service) cleanupProviderGenerations(ctx context.Context, userID, provider, activeGeneration string) error {
	return s.cleanupBatches(ctx, `DELETE FROM search_documents WHERE id IN (
		SELECT id FROM search_documents
		WHERE user_id=? AND provider=? AND generation<>?
		ORDER BY id LIMIT ?
	)`, []any{userID, provider, activeGeneration})
}

func (s *Service) cleanupInactiveGenerations(ctx context.Context) error {
	return s.cleanupBatches(ctx, `DELETE FROM search_documents WHERE id IN (
		SELECT d.id FROM search_documents d
		LEFT JOIN search_provider_generations g
			ON g.user_id=d.user_id AND g.provider=d.provider
		WHERE g.active_generation IS NULL OR d.generation<>g.active_generation
		ORDER BY d.id LIMIT ?
	)`, nil)
}

func (s *Service) cleanupBatches(ctx context.Context, statement string, args []any) error {
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		deleted := 0
		err := s.withConn(ctx, func(conn *sqlite.Conn) error {
			batchArgs := append(append([]any(nil), args...), cleanupBatchSize)
			if err := sqlitex.Execute(conn, statement, &sqlitex.ExecOptions{Args: batchArgs}); err != nil {
				return err
			}
			return sqlitex.Execute(conn, `SELECT changes()`, &sqlitex.ExecOptions{ResultFunc: func(stmt *sqlite.Stmt) error {
				deleted = int(stmt.ColumnInt64(0))
				return nil
			}})
		})
		if err != nil {
			return err
		}
		if deleted < cleanupBatchSize {
			return nil
		}
		timer := time.NewTimer(cleanupYieldDelay)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}

type writer struct {
	service  *Service
	provider string
}

func (w *writer) ReplaceSearchObject(ctx context.Context, userID, objectID string, docs []apps.SearchDocument) error {
	if userID == "" || objectID == "" {
		return errors.New("search user and object id are required")
	}
	for i := range docs {
		if docs[i].ObjectID == "" {
			docs[i].ObjectID = objectID
		}
		if docs[i].ObjectID != objectID {
			return errors.New("search document object id mismatch")
		}
		if err := validateDocument(docs[i]); err != nil {
			return err
		}
	}
	return w.service.withConn(ctx, func(conn *sqlite.Conn) error {
		if err := sqlitex.Execute(conn, `BEGIN IMMEDIATE`, nil); err != nil {
			return err
		}
		committed := false
		defer func() {
			if !committed {
				_ = sqlitex.Execute(conn, `ROLLBACK`, nil)
			}
		}()
		generation, err := ensureActiveGeneration(conn, userID, w.provider)
		if err != nil {
			return err
		}
		if err := sqlitex.Execute(conn, `DELETE FROM search_documents WHERE user_id=? AND provider=? AND generation=? AND object_id=?`,
			&sqlitex.ExecOptions{Args: []any{userID, w.provider, generation, objectID}}); err != nil {
			return err
		}
		for _, doc := range docs {
			if err := insertDocument(conn, userID, w.provider, generation, doc); err != nil {
				return err
			}
		}
		if err := sqlitex.Execute(conn, `COMMIT`, nil); err != nil {
			return err
		}
		committed = true
		return nil
	})
}

func (w *writer) DeleteSearchObject(ctx context.Context, userID, objectID string) error {
	return w.service.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `DELETE FROM search_documents
			WHERE user_id=? AND provider=? AND object_id=?
			AND generation=(SELECT active_generation FROM search_provider_generations WHERE user_id=? AND provider=?)`,
			&sqlitex.ExecOptions{Args: []any{userID, w.provider, objectID, userID, w.provider}})
	})
}

func (w *writer) MarkSearchDirty(provider string) {
	if provider == "" {
		provider = w.provider
	}
	select {
	case w.service.dirty <- provider:
	default:
	}
}

func ensureActiveGeneration(conn *sqlite.Conn, userID, provider string) (string, error) {
	var generation string
	err := sqlitex.Execute(conn, `SELECT active_generation FROM search_provider_generations WHERE user_id=? AND provider=?`,
		&sqlitex.ExecOptions{Args: []any{userID, provider}, ResultFunc: func(stmt *sqlite.Stmt) error {
			generation = stmt.ColumnText(0)
			return nil
		}})
	if err != nil {
		return "", err
	}
	if generation != "" {
		return generation, nil
	}
	generation = randomID()
	if err := sqlitex.Execute(conn, `INSERT INTO search_provider_generations(user_id,provider,active_generation) VALUES(?,?,?)`,
		&sqlitex.ExecOptions{Args: []any{userID, provider, generation}}); err != nil {
		return "", err
	}
	return generation, nil
}

func insertDocument(conn *sqlite.Conn, userID, provider, generation string, doc apps.SearchDocument) error {
	return sqlitex.Execute(conn, `INSERT INTO search_documents(
		user_id,provider,generation,object_id,chunk_id,title,subtitle,keywords,body,url,anchor,modified_at
	) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{
		userID, provider, generation, doc.ObjectID, doc.ChunkID, doc.Title, doc.Subtitle,
		doc.Keywords, doc.Body, doc.URL, doc.Anchor, doc.ModifiedAt,
	}})
}

func validateDocument(doc apps.SearchDocument) error {
	if doc.ObjectID == "" || doc.ChunkID == "" || strings.TrimSpace(doc.Title) == "" || !strings.HasPrefix(doc.URL, "/") || strings.HasPrefix(doc.URL, "//") {
		return errors.New("invalid search document")
	}
	return nil
}

func (s *Service) Preferences(ctx context.Context, userID string) (Preferences, error) {
	defaults := s.defaultPreferences()
	var stored struct {
		Providers []struct {
			Name    string `json:"name"`
			Enabled bool   `json:"enabled"`
		} `json:"providers"`
	}
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		_, err := preferences.GetJSON(conn, userID, "search", "providers", &stored)
		return err
	})
	if err != nil {
		return defaults, err
	}
	known := make(map[string]Provider, len(defaults.Providers))
	for _, provider := range defaults.Providers {
		known[provider.Name] = provider
	}
	out := Preferences{}
	seen := map[string]bool{}
	for _, item := range stored.Providers {
		provider, ok := known[item.Name]
		if !ok || seen[item.Name] {
			continue
		}
		provider.Enabled = item.Enabled
		out.Providers = append(out.Providers, provider)
		seen[item.Name] = true
	}
	for _, provider := range defaults.Providers {
		if !seen[provider.Name] {
			out.Providers = append(out.Providers, provider)
		}
	}
	return out, nil
}

func (s *Service) SavePreferences(ctx context.Context, userID string, ordered []Provider) (Preferences, error) {
	defaults := s.defaultPreferences()
	known := map[string]bool{}
	for _, provider := range defaults.Providers {
		known[provider.Name] = true
	}
	if len(ordered) != len(defaults.Providers) {
		return Preferences{}, errors.New("every search provider must be supplied exactly once")
	}
	seen := map[string]bool{}
	stored := struct {
		Providers []struct {
			Name    string `json:"name"`
			Enabled bool   `json:"enabled"`
		} `json:"providers"`
	}{}
	for _, provider := range ordered {
		if !known[provider.Name] || seen[provider.Name] {
			return Preferences{}, errors.New("invalid or duplicate search provider")
		}
		seen[provider.Name] = true
		stored.Providers = append(stored.Providers, struct {
			Name    string `json:"name"`
			Enabled bool   `json:"enabled"`
		}{Name: provider.Name, Enabled: provider.Enabled})
	}
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return preferences.SetJSON(conn, userID, "search", "providers", stored)
	})
	if err != nil {
		return Preferences{}, err
	}
	return s.Preferences(ctx, userID)
}

func (s *Service) defaultPreferences() Preferences {
	out := Preferences{}
	for _, app := range s.registry.Apps() {
		if _, ok := app.(apps.SearchIndexer); !ok {
			continue
		}
		out.Providers = append(out.Providers, Provider{
			Name: app.Name(), DisplayName: app.DisplayName(), Icon: app.Icon(), Enabled: true,
		})
	}
	return out
}

func (s *Service) Query(ctx context.Context, userID, query, view, onlyProvider, cursor string) (QueryResponse, error) {
	prefs, err := s.Preferences(ctx, userID)
	if err != nil {
		return QueryResponse{}, err
	}
	if view != "suggestions" && view != "results" {
		return QueryResponse{}, errors.New("view must be suggestions or results")
	}
	limit := pageLimit
	if view == "suggestions" {
		limit = suggestLimit
	}
	offset := decodeCursor(cursor)
	match := ftsQuery(query)
	response := QueryResponse{Recent: []Result{}, Groups: []Group{}}
	enabled := make(map[string]bool, len(prefs.Providers))
	for _, provider := range prefs.Providers {
		if provider.Enabled {
			enabled[provider.Name] = true
		}
	}
	var recent []Result
	if len(enabled) > 0 {
		recent, err = s.queryResults(ctx, userID, "", match, recentLimit, 0, true, nil, enabled)
		if err != nil {
			return QueryResponse{}, err
		}
	}
	response.Recent = recent
	for _, provider := range prefs.Providers {
		if !provider.Enabled || (onlyProvider != "" && provider.Name != onlyProvider) {
			continue
		}
		items, err := s.queryResults(ctx, userID, provider.Name, match, limit+1, offset, false, nil, nil)
		if err != nil {
			return QueryResponse{}, err
		}
		group := Group{Provider: provider.Name, DisplayName: provider.DisplayName, Icon: provider.Icon, Results: items}
		if len(group.Results) > limit {
			group.Results = group.Results[:limit]
			group.NextCursor = encodeCursor(offset + limit)
		}
		if len(group.Results) > 0 || view == "results" {
			response.Groups = append(response.Groups, group)
		}
	}
	response.StaleProviders, _ = s.staleProviders(ctx, userID)
	return response, nil
}

func (s *Service) queryResults(ctx context.Context, userID, provider, match string, limit, offset int, recent bool, excluded, allowed map[string]bool) ([]Result, error) {
	out := []Result{}
	args := []any{userID}
	where := `d.user_id=?`
	join := ` JOIN search_provider_generations g
		ON g.user_id=d.user_id AND g.provider=d.provider AND g.active_generation=d.generation`
	order := `d.modified_at DESC,d.id DESC`
	score := "0"
	snippet := `substr(d.body,1,240)`
	if provider != "" {
		where += ` AND d.provider=?`
		args = append(args, provider)
	}
	if len(allowed) > 0 {
		names := make([]string, 0, len(allowed))
		for name := range allowed {
			names = append(names, name)
		}
		sort.Strings(names)
		where += ` AND d.provider IN (` + strings.TrimRight(strings.Repeat("?,", len(names)), ",") + `)`
		for _, name := range names {
			args = append(args, name)
		}
	}
	if match != "" {
		join += ` JOIN search_fts ON search_fts.rowid=d.id`
		where += ` AND search_fts MATCH ?`
		args = append(args, match)
		score = `bm25(search_fts,10.0,5.0,1.0)`
		snippet = `snippet(search_fts,2,'<mark>','</mark>',' … ',24)`
		if !recent {
			order = `score ASC,d.modified_at DESC,d.id DESC`
		}
	}
	// Fetch extra parents because chunk rows collapse below.
	args = append(args, limit*8+32, offset)
	sql := `SELECT d.provider,d.object_id,d.chunk_id,d.title,d.subtitle,d.url,d.anchor,d.modified_at,` + snippet + ` AS snippet,` + score + ` AS score
		FROM search_documents d` + join + ` WHERE ` + where + `
		ORDER BY ` + order + ` LIMIT ? OFFSET ?`
	type key struct{ provider, object string }
	index := map[key]int{}
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, sql, &sqlitex.ExecOptions{Args: args, ResultFunc: func(stmt *sqlite.Stmt) error {
			k := key{stmt.ColumnText(0), stmt.ColumnText(1)}
			if excluded != nil && excluded[k.provider+"\x00"+k.object] {
				return nil
			}
			if at, ok := index[k]; ok {
				out[at].MatchCount++
				return nil
			}
			if len(out) >= limit {
				return nil
			}
			result := Result{
				Provider: k.provider, ObjectID: k.object, Title: stmt.ColumnText(3),
				Subtitle: stmt.ColumnText(4), URL: stmt.ColumnText(5), ModifiedAt: stmt.ColumnInt64(7),
				Match:      Match{ChunkID: stmt.ColumnText(2), Snippet: safeSnippet(stmt.ColumnText(8)), Anchor: stmt.ColumnText(6)},
				MatchCount: 1,
			}
			index[k] = len(out)
			out = append(out, result)
			return nil
		}})
	})
	return out, err
}

func (s *Service) Matches(ctx context.Context, userID, provider, objectID, query string) ([]Match, error) {
	preferences, err := s.Preferences(ctx, userID)
	if err != nil {
		return nil, err
	}
	enabled := false
	for _, item := range preferences.Providers {
		if item.Name == provider {
			enabled = item.Enabled
			break
		}
	}
	if !enabled {
		return []Match{}, nil
	}
	match := ftsQuery(query)
	if match == "" {
		return []Match{}, nil
	}
	out := []Match{}
	err = s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT d.chunk_id,snippet(search_fts,2,'<mark>','</mark>',' … ',32),d.anchor
			FROM search_documents d
			JOIN search_provider_generations g
				ON g.user_id=d.user_id AND g.provider=d.provider AND g.active_generation=d.generation
			JOIN search_fts ON search_fts.rowid=d.id
			WHERE d.user_id=? AND d.provider=? AND d.object_id=? AND search_fts MATCH ?
			ORDER BY bm25(search_fts,10.0,5.0,1.0),d.modified_at DESC LIMIT 50`,
			&sqlitex.ExecOptions{Args: []any{userID, provider, objectID, match}, ResultFunc: func(stmt *sqlite.Stmt) error {
				out = append(out, Match{ChunkID: stmt.ColumnText(0), Snippet: safeSnippet(stmt.ColumnText(1)), Anchor: stmt.ColumnText(2)})
				return nil
			}})
	})
	return out, err
}

func (s *Service) staleProviders(ctx context.Context, userID string) ([]string, error) {
	var out []string
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT provider FROM search_provider_runs WHERE user_id=? AND last_error_at>last_success_at ORDER BY provider`,
			&sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				out = append(out, stmt.ColumnText(0))
				return nil
			}})
	})
	return out, err
}

func (s *Service) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	conn, release, err := s.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(conn)
}

type queryRequest struct {
	Query    string `json:"query"`
	View     string `json:"view"`
	Provider string `json:"provider,omitempty"`
	Cursor   string `json:"cursor,omitempty"`
}

type matchesRequest struct {
	Query    string `json:"query"`
	Provider string `json:"provider"`
	ObjectID string `json:"object_id"`
}

func (s *Service) HandleQuery(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	user := server.GetUser(r)
	var request queryRequest
	if !decode(w, r, &request) {
		return
	}
	if len(request.Query) > 500 || len(request.Cursor) > 32 {
		http.Error(w, `{"error":"invalid query"}`, http.StatusBadRequest)
		return
	}
	response, err := s.Query(r.Context(), user.ID, request.Query, request.View, request.Provider, request.Cursor)
	if err != nil {
		http.Error(w, `{"error":"search failed"}`, http.StatusInternalServerError)
		return
	}
	writeJSON(w, response)
}

func (s *Service) HandleMatches(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	user := server.GetUser(r)
	var request matchesRequest
	if !decode(w, r, &request) {
		return
	}
	matches, err := s.Matches(r.Context(), user.ID, request.Provider, request.ObjectID, request.Query)
	if err != nil {
		http.Error(w, `{"error":"search failed"}`, http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{"matches": matches})
}

func (s *Service) HandlePreferences(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	switch r.Method {
	case http.MethodGet:
		value, err := s.Preferences(r.Context(), user.ID)
		if err != nil {
			http.Error(w, `{"error":"preferences failed"}`, http.StatusInternalServerError)
			return
		}
		writeJSON(w, value)
	case http.MethodPut:
		var request Preferences
		if !decode(w, r, &request) {
			return
		}
		value, err := s.SavePreferences(r.Context(), user.ID, request.Providers)
		if err != nil {
			http.Error(w, `{"error":"invalid preferences"}`, http.StatusBadRequest)
			return
		}
		writeJSON(w, value)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func decode(w http.ResponseWriter, r *http.Request, value any) bool {
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(value); err != nil {
		http.Error(w, `{"error":"invalid json"}`, http.StatusBadRequest)
		return false
	}
	return true
}

func writeJSON(w http.ResponseWriter, value any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(value)
}

// ChunkText splits UTF-8 text near logical boundaries while retaining a small
// overlap for cross-boundary queries.
func ChunkText(text string) []string {
	const target = 16 << 10
	const overlap = 1 << 10
	text = strings.TrimSpace(text)
	if text == "" {
		return []string{""}
	}
	if len(text) <= target {
		return []string{text}
	}
	var chunks []string
	start := 0
	for start < len(text) {
		end := start + target
		if end >= len(text) {
			chunks = append(chunks, strings.TrimSpace(text[start:]))
			break
		}
		for end > start+target/2 && text[end] != '\n' && text[end] != ' ' {
			end--
		}
		for end > start && !utf8.RuneStart(text[end]) {
			end--
		}
		if end <= start {
			end = start + target
		}
		chunks = append(chunks, strings.TrimSpace(text[start:end]))
		next := end - overlap
		for next > start && !utf8.RuneStart(text[next]) {
			next--
		}
		if next <= start {
			next = end
		}
		start = next
	}
	return chunks
}

func ftsQuery(value string) string {
	var tokens []string
	var current strings.Builder
	flush := func() {
		if current.Len() == 0 {
			return
		}
		token := strings.ReplaceAll(current.String(), `"`, `""`)
		tokens = append(tokens, `"`+token+`"*`)
		current.Reset()
	}
	for _, r := range strings.TrimSpace(value) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_' {
			current.WriteRune(r)
		} else {
			flush()
		}
	}
	flush()
	return strings.Join(tokens, " AND ")
}

func safeSnippet(value string) string {
	const open = "\x00OPEN\x00"
	const close = "\x00CLOSE\x00"
	value = strings.ReplaceAll(value, "<mark>", open)
	value = strings.ReplaceAll(value, "</mark>", close)
	value = html.EscapeString(value)
	value = strings.ReplaceAll(value, open, "<mark>")
	return strings.ReplaceAll(value, close, "</mark>")
}

func encodeCursor(value int) string { return strconv.Itoa(value) }
func decodeCursor(value string) int {
	n, _ := strconv.Atoi(value)
	if n < 0 {
		return 0
	}
	return n
}

func randomID() string {
	var raw [16]byte
	_, _ = rand.Read(raw[:])
	return hex.EncodeToString(raw[:])
}

func truncate(value string, n int) string {
	if len(value) <= n {
		return value
	}
	return value[:n]
}

// SortProviders is used by tests and UIs that need the registry order without
// mutating stored preferences.
func SortProviders(providers []Provider) {
	sort.SliceStable(providers, func(i, j int) bool { return providers[i].Name < providers[j].Name })
}
