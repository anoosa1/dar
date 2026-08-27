// Package preferences stores small per-user app preferences as JSON values.
package preferences

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS user_preferences (
		user_id TEXT NOT NULL,
		app TEXT NOT NULL,
		key TEXT NOT NULL,
		value_json TEXT NOT NULL,
		updated_at INTEGER NOT NULL DEFAULT 0,
		PRIMARY KEY(user_id, app, key)
	);`,
}

// Migrations returns the SQL migrations for user preferences.
func Migrations() []string {
	return migrations
}

// GetJSON decodes one preference value into dst. It returns false when unset.
func GetJSON(conn *sqlite.Conn, userID, app, key string, dst any) (bool, error) {
	if strings.TrimSpace(userID) == "" || strings.TrimSpace(app) == "" || strings.TrimSpace(key) == "" {
		return false, fmt.Errorf("user id, app, and key required")
	}
	var raw string
	err := sqlitex.Execute(conn,
		`SELECT value_json FROM user_preferences WHERE user_id = ? AND app = ? AND key = ?`,
		&sqlitex.ExecOptions{
			Args: []any{userID, app, key},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				raw = stmt.ColumnText(0)
				return nil
			},
		},
	)
	if err != nil {
		return false, fmt.Errorf("get preference: %w", err)
	}
	if raw == "" {
		return false, nil
	}
	if err := json.Unmarshal([]byte(raw), dst); err != nil {
		return false, fmt.Errorf("decode preference: %w", err)
	}
	return true, nil
}

// SetJSON encodes and upserts one preference value.
func SetJSON(conn *sqlite.Conn, userID, app, key string, value any) error {
	if strings.TrimSpace(userID) == "" || strings.TrimSpace(app) == "" || strings.TrimSpace(key) == "" {
		return fmt.Errorf("user id, app, and key required")
	}
	data, err := json.Marshal(value)
	if err != nil {
		return fmt.Errorf("encode preference: %w", err)
	}
	err = sqlitex.Execute(conn,
		`INSERT INTO user_preferences (user_id, app, key, value_json, updated_at)
		 VALUES (?, ?, ?, ?, ?)
		 ON CONFLICT(user_id, app, key) DO UPDATE SET
			value_json = excluded.value_json,
			updated_at = excluded.updated_at`,
		&sqlitex.ExecOptions{Args: []any{userID, app, key, string(data), time.Now().Unix()}},
	)
	if err != nil {
		return fmt.Errorf("set preference: %w", err)
	}
	return nil
}
