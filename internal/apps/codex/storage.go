package codex

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"github.com/anoosa/dar/internal/db"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS codex_sessions (
		id TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id),
		thread_id TEXT NOT NULL,
		title TEXT NOT NULL DEFAULT 'New chat',
		model TEXT NOT NULL DEFAULT '',
		reasoning_effort TEXT NOT NULL DEFAULT '',
		archived INTEGER NOT NULL DEFAULT 0,
		created_at INTEGER NOT NULL,
		updated_at INTEGER NOT NULL,
		UNIQUE(user_id, thread_id)
	);
	CREATE INDEX IF NOT EXISTS idx_codex_sessions_user_updated ON codex_sessions(user_id, archived, updated_at DESC);`,
	`CREATE TABLE IF NOT EXISTS codex_preferences (
		user_id TEXT PRIMARY KEY REFERENCES users(id),
		approval_mode TEXT NOT NULL DEFAULT 'confirm',
		web_search_mode TEXT NOT NULL DEFAULT 'disabled',
		updated_at INTEGER NOT NULL DEFAULT 0
	);`,
}

func Migrations() []string { return append([]string(nil), migrations...) }

type Session struct {
	ID              string `json:"id"`
	ThreadID        string `json:"-"`
	Title           string `json:"title"`
	Model           string `json:"model,omitempty"`
	ReasoningEffort string `json:"reasoning_effort,omitempty"`
	Archived        bool   `json:"archived"`
	CreatedAt       int64  `json:"created_at"`
	UpdatedAt       int64  `json:"updated_at"`
}

type Preferences struct {
	ApprovalMode string `json:"approval_mode"`
	WebSearch    string `json:"web_search_mode"`
}

func defaultPreferences() Preferences {
	return Preferences{ApprovalMode: "confirm", WebSearch: "disabled"}
}

type store struct{ db *db.DB }

func (s store) createSession(ctx context.Context, userID, threadID, model, effort string) (Session, error) {
	now := time.Now().Unix()
	session := Session{ID: randomID(), ThreadID: threadID, Title: "New chat", Model: model, ReasoningEffort: effort, CreatedAt: now, UpdatedAt: now}
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `INSERT INTO codex_sessions(id,user_id,thread_id,title,model,reasoning_effort,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{session.ID, userID, threadID, session.Title, model, effort, now, now}})
	})
	return session, err
}

func (s store) listSessions(ctx context.Context, userID string, archived bool) ([]Session, error) {
	var out []Session
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT id,thread_id,title,model,reasoning_effort,archived,created_at,updated_at FROM codex_sessions WHERE user_id=? AND archived=? ORDER BY updated_at DESC`, &sqlitex.ExecOptions{Args: []any{userID, boolInt(archived)}, ResultFunc: func(stmt *sqlite.Stmt) error {
			out = append(out, scanSession(stmt))
			return nil
		}})
	})
	return out, err
}

func (s store) getSession(ctx context.Context, userID, id string) (Session, error) {
	var out Session
	found := false
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT id,thread_id,title,model,reasoning_effort,archived,created_at,updated_at FROM codex_sessions WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{userID, id}, ResultFunc: func(stmt *sqlite.Stmt) error {
			out, found = scanSession(stmt), true
			return nil
		}})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return out, err
}

func (s store) sessionByThread(ctx context.Context, userID, threadID string) (Session, error) {
	var out Session
	found := false
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT id,thread_id,title,model,reasoning_effort,archived,created_at,updated_at FROM codex_sessions WHERE user_id=? AND thread_id=?`, &sqlitex.ExecOptions{Args: []any{userID, threadID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			out, found = scanSession(stmt), true
			return nil
		}})
	})
	if err == nil && !found {
		err = errNotFound
	}
	return out, err
}

func (s store) updateSession(ctx context.Context, userID, id, title, model, effort string, archived *bool) error {
	session, err := s.getSession(ctx, userID, id)
	if err != nil {
		return err
	}
	if title != "" {
		session.Title = title
	}
	if model != "" {
		session.Model = model
	}
	if effort != "" {
		session.ReasoningEffort = effort
	}
	if archived != nil {
		session.Archived = *archived
	}
	return s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `UPDATE codex_sessions SET title=?,model=?,reasoning_effort=?,archived=?,updated_at=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{session.Title, session.Model, session.ReasoningEffort, boolInt(session.Archived), time.Now().Unix(), userID, id}})
	})
}

func (s store) touchSession(ctx context.Context, userID, threadID string) {
	_ = s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `UPDATE codex_sessions SET updated_at=? WHERE user_id=? AND thread_id=?`, &sqlitex.ExecOptions{Args: []any{time.Now().Unix(), userID, threadID}})
	})
}

func (s store) deleteSession(ctx context.Context, userID, id string) error {
	return s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `DELETE FROM codex_sessions WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{userID, id}})
	})
}

func (s store) preferences(ctx context.Context, userID string) (Preferences, error) {
	out := defaultPreferences()
	err := s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT approval_mode,web_search_mode FROM codex_preferences WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			out.ApprovalMode, out.WebSearch = stmt.ColumnText(0), stmt.ColumnText(1)
			return nil
		}})
	})
	return out, err
}

func (s store) savePreferences(ctx context.Context, userID string, p Preferences) error {
	if p.ApprovalMode != "confirm" && p.ApprovalMode != "automatic" {
		return fmt.Errorf("invalid approval mode")
	}
	if p.WebSearch != "disabled" && p.WebSearch != "cached" && p.WebSearch != "live" {
		return fmt.Errorf("invalid web search mode")
	}
	return s.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `INSERT INTO codex_preferences(user_id,approval_mode,web_search_mode,updated_at) VALUES(?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET approval_mode=excluded.approval_mode,web_search_mode=excluded.web_search_mode,updated_at=excluded.updated_at`, &sqlitex.ExecOptions{Args: []any{userID, p.ApprovalMode, p.WebSearch, time.Now().Unix()}})
	})
}

func (s store) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	conn, release, err := s.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(conn)
}

func scanSession(stmt *sqlite.Stmt) Session {
	return Session{ID: stmt.ColumnText(0), ThreadID: stmt.ColumnText(1), Title: stmt.ColumnText(2), Model: stmt.ColumnText(3), ReasoningEffort: stmt.ColumnText(4), Archived: stmt.ColumnInt64(5) != 0, CreatedAt: stmt.ColumnInt64(6), UpdatedAt: stmt.ColumnInt64(7)}
}

var errNotFound = errors.New("not found")

func randomID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}

func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}
