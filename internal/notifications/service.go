// Package notifications provides Dar's headless, user-scoped notification
// center and live delivery stream.
package notifications

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS notifications (
		id TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		source TEXT NOT NULL,
		kind TEXT NOT NULL,
		subject TEXT NOT NULL,
		message TEXT NOT NULL DEFAULT '',
		icon TEXT NOT NULL DEFAULT '',
		object_url TEXT NOT NULL DEFAULT '',
		dedupe_key TEXT NOT NULL DEFAULT '',
		created_at INTEGER NOT NULL,
		dismissed_at INTEGER NOT NULL DEFAULT 0
	);
	CREATE INDEX IF NOT EXISTS idx_notifications_user_active
		ON notifications(user_id,dismissed_at,created_at DESC,id DESC);
	CREATE UNIQUE INDEX IF NOT EXISTS idx_notifications_dedupe
		ON notifications(user_id,source,dedupe_key) WHERE dedupe_key<>'';`,
}

// Migrations returns the notification schema.
func Migrations() []string { return append([]string(nil), migrations...) }

type streamEvent struct {
	name string
	data any
}

// Service persists notifications and fans committed changes out to connected
// browser sessions. It intentionally has no app-launcher entry.
type Service struct {
	db          *db.DB
	mu          sync.Mutex
	subscribers map[string]map[chan streamEvent]struct{}
	closeOnce   sync.Once
	closed      chan struct{}
}

func New(database *db.DB) *Service {
	return &Service{
		db: database, subscribers: make(map[string]map[chan streamEvent]struct{}),
		closed: make(chan struct{}),
	}
}

// Close disconnects live streams during HTTP server shutdown.
func (s *Service) Close() {
	s.closeOnce.Do(func() { close(s.closed) })
}

func newID() (string, error) {
	var data [16]byte
	if _, err := rand.Read(data[:]); err != nil {
		return "", err
	}
	return "ntf_" + hex.EncodeToString(data[:]), nil
}

func (s *Service) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if s.db == nil {
		return errors.New("notification database is unavailable")
	}
	conn, release, err := s.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(conn)
}

// EnqueueNotification implements apps.NotificationRecorder.
func (s *Service) EnqueueNotification(ctx context.Context, notification apps.Notification) error {
	if notification.ID == "" {
		id, err := newID()
		if err != nil {
			return fmt.Errorf("create notification id: %w", err)
		}
		notification.ID = id
	}
	if notification.CreatedAt <= 0 {
		notification.CreatedAt = time.Now().Unix()
	}
	inserted := false
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`INSERT INTO notifications(id,user_id,source,kind,subject,message,icon,object_url,dedupe_key,created_at)
			 VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT DO NOTHING RETURNING id`,
			&sqlitex.ExecOptions{
				Args: []any{
					notification.ID, notification.RecipientID, notification.Source, notification.Kind,
					notification.Subject, notification.Message, notification.Icon,
					notification.ObjectURL, notification.DedupeKey, notification.CreatedAt,
				},
				ResultFunc: func(*sqlite.Stmt) error {
					inserted = true
					return nil
				},
			},
		)
	})
	if err != nil {
		return fmt.Errorf("enqueue notification: %w", err)
	}
	if inserted {
		s.publish(notification.RecipientID, streamEvent{name: "notification.created", data: storedNotification{
			ID: notification.ID, Source: notification.Source, Kind: notification.Kind,
			Subject: notification.Subject, Message: notification.Message, Icon: notification.Icon,
			ObjectURL: notification.ObjectURL, CreatedAt: notification.CreatedAt,
		}})
	}
	return nil
}

func (s *Service) subscribe(userID string) (<-chan streamEvent, func()) {
	ch := make(chan streamEvent, 64)
	s.mu.Lock()
	if s.subscribers[userID] == nil {
		s.subscribers[userID] = make(map[chan streamEvent]struct{})
	}
	s.subscribers[userID][ch] = struct{}{}
	s.mu.Unlock()
	return ch, func() {
		s.mu.Lock()
		delete(s.subscribers[userID], ch)
		if len(s.subscribers[userID]) == 0 {
			delete(s.subscribers, userID)
		}
		s.mu.Unlock()
	}
}

func (s *Service) publish(userID string, event streamEvent) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for ch := range s.subscribers[userID] {
		select {
		case ch <- event:
		default:
			// The stream sends periodic resync events, so a slow browser can
			// safely recover without blocking notification producers.
		}
	}
}

type storedNotification struct {
	ID        string `json:"id"`
	Source    string `json:"source"`
	Kind      string `json:"kind"`
	Subject   string `json:"subject"`
	Message   string `json:"message,omitempty"`
	Icon      string `json:"icon,omitempty"`
	ObjectURL string `json:"object_url,omitempty"`
	CreatedAt int64  `json:"created_at"`
}

func scanNotification(stmt *sqlite.Stmt) storedNotification {
	return storedNotification{
		ID: stmt.ColumnText(0), Source: stmt.ColumnText(1), Kind: stmt.ColumnText(2),
		Subject: stmt.ColumnText(3), Message: stmt.ColumnText(4), Icon: stmt.ColumnText(5),
		ObjectURL: stmt.ColumnText(6), CreatedAt: stmt.ColumnInt64(7),
	}
}

func (s *Service) list(ctx context.Context, userID string, limit int) ([]storedNotification, int, error) {
	if limit < 1 || limit > 100 {
		limit = 50
	}
	var items []storedNotification
	count := 0
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		if err := sqlitex.Execute(conn,
			`SELECT id,source,kind,subject,message,icon,object_url,created_at
			 FROM notifications WHERE user_id=? AND dismissed_at=0
			 ORDER BY created_at DESC,id DESC LIMIT ?`,
			&sqlitex.ExecOptions{Args: []any{userID, limit}, ResultFunc: func(stmt *sqlite.Stmt) error {
				items = append(items, scanNotification(stmt))
				return nil
			}},
		); err != nil {
			return err
		}
		return sqlitex.Execute(conn,
			`SELECT count(*) FROM notifications WHERE user_id=? AND dismissed_at=0`,
			&sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				count = int(stmt.ColumnInt64(0))
				return nil
			}},
		)
	})
	if items == nil {
		items = []storedNotification{}
	}
	return items, count, err
}

func (s *Service) dismiss(ctx context.Context, userID, id string) (bool, error) {
	changed := false
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`UPDATE notifications SET dismissed_at=? WHERE id=? AND user_id=? AND dismissed_at=0 RETURNING id`,
			&sqlitex.ExecOptions{Args: []any{time.Now().Unix(), id, userID}, ResultFunc: func(*sqlite.Stmt) error {
				changed = true
				return nil
			}},
		)
	})
	if err == nil && changed {
		s.publish(userID, streamEvent{name: "notification.dismissed", data: map[string]string{"id": id}})
	}
	return changed, err
}

func (s *Service) dismissAll(ctx context.Context, userID string) error {
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn,
			`UPDATE notifications SET dismissed_at=? WHERE user_id=? AND dismissed_at=0`,
			&sqlitex.ExecOptions{Args: []any{time.Now().Unix(), userID}},
		)
	})
	if err == nil {
		s.publish(userID, streamEvent{name: "notification.dismissed_all", data: map[string]bool{"ok": true}})
	}
	return err
}

type notificationPreferences struct {
	DesktopEnabled bool `json:"desktop_enabled"`
}

func (s *Service) loadPreferences(ctx context.Context, userID string) (notificationPreferences, error) {
	var out notificationPreferences
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		_, err := preferences.GetJSON(conn, userID, "notifications", "settings", &out)
		return err
	})
	return out, err
}

func (s *Service) savePreferences(ctx context.Context, userID string, value notificationPreferences) error {
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return preferences.SetJSON(conn, userID, "notifications", "settings", value)
	})
	if err == nil {
		s.publish(userID, streamEvent{name: "notification.preferences", data: value})
	}
	return err
}

func safeEventName(name string) string {
	name = strings.TrimSpace(name)
	if name == "" || strings.ContainsAny(name, "\r\n") {
		return "notification.resync"
	}
	return name
}

func encodeEventData(value any) []byte {
	data, err := json.Marshal(value)
	if err != nil {
		return []byte(`{}`)
	}
	return data
}
