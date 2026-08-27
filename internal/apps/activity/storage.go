package activity

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS activity_events (
		id TEXT PRIMARY KEY,
		actor_id TEXT NOT NULL DEFAULT '',
		actor_name TEXT NOT NULL,
		actor_kind TEXT NOT NULL,
		source TEXT NOT NULL,
		action TEXT NOT NULL,
		summary TEXT NOT NULL,
		object_type TEXT NOT NULL DEFAULT '',
		object_id TEXT NOT NULL DEFAULT '',
		object_name TEXT NOT NULL DEFAULT '',
		object_url TEXT NOT NULL DEFAULT '',
		origin TEXT NOT NULL,
		happened_at INTEGER NOT NULL,
		metadata_json TEXT NOT NULL DEFAULT '{}'
	);
	CREATE INDEX IF NOT EXISTS idx_activity_events_recent ON activity_events(happened_at DESC,id DESC);
	CREATE INDEX IF NOT EXISTS idx_activity_events_object ON activity_events(source,object_type,object_id,happened_at DESC);`,
	`CREATE TABLE IF NOT EXISTS activity_recipients (
		event_id TEXT NOT NULL REFERENCES activity_events(id) ON DELETE CASCADE,
		user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		PRIMARY KEY(event_id,user_id)
	);
	CREATE INDEX IF NOT EXISTS idx_activity_recipients_user ON activity_recipients(user_id,event_id);`,
	`CREATE TABLE IF NOT EXISTS activity_outbox (
		id TEXT PRIMARY KEY,
		payload_json TEXT NOT NULL,
		attempt_count INTEGER NOT NULL DEFAULT 0,
		next_attempt_at INTEGER NOT NULL DEFAULT 0,
		last_error TEXT NOT NULL DEFAULT '',
		created_at INTEGER NOT NULL
	);
	CREATE INDEX IF NOT EXISTS idx_activity_outbox_due ON activity_outbox(next_attempt_at,created_at,id);`,
	`CREATE TABLE IF NOT EXISTS activity_feed_settings (
		user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
		enabled INTEGER NOT NULL DEFAULT 0,
		sources_json TEXT NOT NULL DEFAULT '[]',
		token_hash TEXT NOT NULL DEFAULT '',
		updated_at INTEGER NOT NULL DEFAULT 0
	);
	CREATE UNIQUE INDEX IF NOT EXISTS idx_activity_feed_token ON activity_feed_settings(token_hash) WHERE token_hash<>'';`,
	`DROP TABLE IF EXISTS files_activity;`,
}

func Migrations() []string { return append([]string(nil), migrations...) }

func randomID(prefix string, bytes int) (string, error) {
	buf := make([]byte, bytes)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return prefix + hex.EncodeToString(buf), nil
}

func (a *App) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return errors.New("activity database is not configured")
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(conn)
}

// EnqueueActivity implements apps.ActivityRecorder.
func (a *App) EnqueueActivity(ctx context.Context, event apps.ActivityEvent) error {
	if event.ID == "" {
		var err error
		event.ID, err = randomID("act_", 16)
		if err != nil {
			return fmt.Errorf("create activity id: %w", err)
		}
	}
	if event.HappenedAt <= 0 {
		event.HappenedAt = time.Now().Unix()
	}
	if event.Metadata == nil {
		event.Metadata = map[string]string{}
	}
	payload, err := json.Marshal(event)
	if err != nil {
		return fmt.Errorf("encode activity: %w", err)
	}
	now := time.Now().Unix()
	err = a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`INSERT INTO activity_outbox(id,payload_json,next_attempt_at,created_at) VALUES(?,?,?,?)`,
			&sqlitex.ExecOptions{Args: []any{event.ID, string(payload), now, now}},
		)
	})
	if err != nil {
		return fmt.Errorf("enqueue activity: %w", err)
	}
	select {
	case a.wake <- struct{}{}:
	default:
	}
	return nil
}

func (a *App) runOutbox(ctx context.Context) {
	defer a.wg.Done()
	ticker := time.NewTicker(15 * time.Second)
	defer ticker.Stop()
	for {
		a.drainOutbox(ctx)
		select {
		case <-ctx.Done():
			return
		case <-a.wake:
		case <-ticker.C:
		}
	}
}

type pendingEvent struct {
	id       string
	payload  string
	attempts int
}

func (a *App) drainOutbox(ctx context.Context) {
	for ctx.Err() == nil {
		var pending pendingEvent
		err := a.withConn(ctx, func(conn *sqlite.Conn) error {
			return sqlitex.Execute(conn,
				`SELECT id,payload_json,attempt_count FROM activity_outbox
				 WHERE next_attempt_at<=? ORDER BY created_at,id LIMIT 1`,
				&sqlitex.ExecOptions{Args: []any{time.Now().Unix()}, ResultFunc: func(stmt *sqlite.Stmt) error {
					pending = pendingEvent{id: stmt.ColumnText(0), payload: stmt.ColumnText(1), attempts: int(stmt.ColumnInt64(2))}
					return nil
				}},
			)
		})
		if err != nil || pending.id == "" {
			return
		}
		var event apps.ActivityEvent
		if err = json.Unmarshal([]byte(pending.payload), &event); err == nil {
			err = a.persistEvent(ctx, event)
		}
		if err == nil {
			continue
		}
		attempt := pending.attempts + 1
		delay := time.Second << min(attempt-1, 8)
		if delay > 5*time.Minute {
			delay = 5 * time.Minute
		}
		_ = a.withConn(ctx, func(conn *sqlite.Conn) error {
			return sqlitex.Execute(conn,
				`UPDATE activity_outbox SET attempt_count=?,next_attempt_at=?,last_error=? WHERE id=?`,
				&sqlitex.ExecOptions{Args: []any{attempt, time.Now().Add(delay).Unix(), truncateError(err), pending.id}},
			)
		})
	}
}

func truncateError(err error) string {
	if err == nil {
		return ""
	}
	text := err.Error()
	if len(text) > 500 {
		text = text[:500]
	}
	return text
}

func (a *App) persistEvent(ctx context.Context, event apps.ActivityEvent) error {
	metadata, err := json.Marshal(event.Metadata)
	if err != nil {
		return err
	}
	return a.withConn(ctx, func(conn *sqlite.Conn) (txErr error) {
		end, txErr := sqlitex.ImmediateTransaction(conn)
		if txErr != nil {
			return txErr
		}
		defer end(&txErr)
		if txErr = sqlitex.Execute(conn,
			`INSERT INTO activity_events(id,actor_id,actor_name,actor_kind,source,action,summary,object_type,object_id,object_name,object_url,origin,happened_at,metadata_json)
			 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING`,
			&sqlitex.ExecOptions{Args: []any{event.ID, event.ActorID, event.ActorName, event.ActorKind, event.Source, event.Action, event.Summary, event.ObjectType, event.ObjectID, event.ObjectName, event.ObjectURL, event.Origin, event.HappenedAt, string(metadata)}},
		); txErr != nil {
			return txErr
		}
		for _, userID := range event.RecipientIDs {
			if txErr = sqlitex.Execute(conn,
				`INSERT INTO activity_recipients(event_id,user_id) VALUES(?,?) ON CONFLICT(event_id,user_id) DO NOTHING`,
				&sqlitex.ExecOptions{Args: []any{event.ID, userID}},
			); txErr != nil {
				return txErr
			}
		}
		return sqlitex.Execute(conn, `DELETE FROM activity_outbox WHERE id=?`, &sqlitex.ExecOptions{Args: []any{event.ID}})
	})
}

type cursor struct {
	Time int64  `json:"t"`
	ID   string `json:"i"`
}

func encodeCursor(c cursor) string {
	raw, _ := json.Marshal(c)
	return base64.RawURLEncoding.EncodeToString(raw)
}

func decodeCursor(raw string) (cursor, error) {
	if raw == "" {
		return cursor{}, nil
	}
	data, err := base64.RawURLEncoding.DecodeString(raw)
	if err != nil {
		return cursor{}, errors.New("invalid cursor")
	}
	var out cursor
	if err = json.Unmarshal(data, &out); err != nil || out.Time <= 0 || out.ID == "" {
		return cursor{}, errors.New("invalid cursor")
	}
	return out, nil
}

type Event struct {
	ID         string            `json:"id"`
	ActorID    string            `json:"actor_id,omitempty"`
	ActorName  string            `json:"actor_name"`
	ActorKind  string            `json:"actor_kind"`
	ActorIsYou bool              `json:"actor_is_you"`
	Source     string            `json:"source"`
	Action     string            `json:"action"`
	Summary    string            `json:"summary"`
	ObjectType string            `json:"object_type,omitempty"`
	ObjectID   string            `json:"object_id,omitempty"`
	ObjectName string            `json:"object_name,omitempty"`
	ObjectURL  string            `json:"object_url,omitempty"`
	Origin     string            `json:"origin"`
	HappenedAt int64             `json:"happened_at"`
	Metadata   map[string]string `json:"metadata,omitempty"`
}

type listOptions struct {
	UserID     string
	Source     string
	ObjectType string
	ObjectID   string
	Cursor     cursor
	Limit      int
	Sources    []string
}

func (a *App) listEvents(ctx context.Context, opts listOptions) ([]Event, string, error) {
	limit := opts.Limit
	if limit <= 0 || limit > 100 {
		limit = 50
	}
	args := []any{opts.UserID}
	where := []string{"r.user_id=?"}
	if opts.Source != "" && opts.Source != "all" {
		where = append(where, "e.source=?")
		args = append(args, opts.Source)
	}
	if len(opts.Sources) > 0 {
		placeholders := make([]string, len(opts.Sources))
		for i, source := range opts.Sources {
			placeholders[i] = "?"
			args = append(args, source)
		}
		where = append(where, "e.source IN ("+strings.Join(placeholders, ",")+")")
	}
	if opts.ObjectType != "" {
		where = append(where, "e.object_type=?")
		args = append(args, opts.ObjectType)
	}
	if opts.ObjectID != "" {
		where = append(where, "e.object_id=?")
		args = append(args, opts.ObjectID)
	}
	if opts.Cursor.Time > 0 {
		where = append(where, "(e.happened_at<? OR (e.happened_at=? AND e.id<?))")
		args = append(args, opts.Cursor.Time, opts.Cursor.Time, opts.Cursor.ID)
	}
	args = append(args, limit+1)
	query := `SELECT e.id,e.actor_id,e.actor_name,e.actor_kind,e.source,e.action,e.summary,e.object_type,e.object_id,e.object_name,e.object_url,e.origin,e.happened_at,e.metadata_json
		FROM activity_events e JOIN activity_recipients r ON r.event_id=e.id
		WHERE ` + strings.Join(where, " AND ") + ` ORDER BY e.happened_at DESC,e.id DESC LIMIT ?`
	var events []Event
	err := a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, query, &sqlitex.ExecOptions{Args: args, ResultFunc: func(stmt *sqlite.Stmt) error {
			event := Event{
				ID: stmt.ColumnText(0), ActorID: stmt.ColumnText(1), ActorName: stmt.ColumnText(2),
				ActorKind: stmt.ColumnText(3), Source: stmt.ColumnText(4), Action: stmt.ColumnText(5),
				Summary: stmt.ColumnText(6), ObjectType: stmt.ColumnText(7), ObjectID: stmt.ColumnText(8),
				ObjectName: stmt.ColumnText(9), ObjectURL: stmt.ColumnText(10), Origin: stmt.ColumnText(11),
				HappenedAt: stmt.ColumnInt64(12),
			}
			event.ActorIsYou = event.ActorID != "" && event.ActorID == opts.UserID
			_ = json.Unmarshal([]byte(stmt.ColumnText(13)), &event.Metadata)
			events = append(events, event)
			return nil
		}})
	})
	if err != nil {
		return nil, "", err
	}
	next := ""
	if len(events) > limit {
		last := events[limit-1]
		next = encodeCursor(cursor{Time: last.HappenedAt, ID: last.ID})
		events = events[:limit]
	}
	return events, next, nil
}

type feedSettings struct {
	Enabled   bool     `json:"rss_enabled"`
	Sources   []string `json:"sources"`
	TokenHash string   `json:"-"`
}

func (a *App) loadFeedSettings(ctx context.Context, userID string) (feedSettings, bool, error) {
	var out feedSettings
	found := false
	err := a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`SELECT enabled,sources_json,token_hash FROM activity_feed_settings WHERE user_id=?`,
			&sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				found = true
				out.Enabled = stmt.ColumnInt64(0) != 0
				_ = json.Unmarshal([]byte(stmt.ColumnText(1)), &out.Sources)
				out.TokenHash = stmt.ColumnText(2)
				return nil
			}},
		)
	})
	return out, found, err
}

func tokenHash(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

func newFeedToken() (string, string, error) {
	token, err := randomID("dar_activity_", 32)
	if err != nil {
		return "", "", err
	}
	return token, tokenHash(token), nil
}

func (a *App) saveFeedSettings(ctx context.Context, userID string, settings feedSettings) error {
	raw, err := json.Marshal(settings.Sources)
	if err != nil {
		return err
	}
	return a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`INSERT INTO activity_feed_settings(user_id,enabled,sources_json,token_hash,updated_at) VALUES(?,?,?,?,?)
			 ON CONFLICT(user_id) DO UPDATE SET enabled=excluded.enabled,sources_json=excluded.sources_json,token_hash=excluded.token_hash,updated_at=excluded.updated_at`,
			&sqlitex.ExecOptions{Args: []any{userID, boolInt(settings.Enabled), string(raw), settings.TokenHash, time.Now().Unix()}},
		)
	})
}

func boolInt(value bool) int {
	if value {
		return 1
	}
	return 0
}
