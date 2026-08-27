package prayer

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS prayer_occurrences (
		user_id TEXT NOT NULL, local_date TEXT NOT NULL, prayer TEXT NOT NULL, external_id TEXT NOT NULL,
		scheduled_at INTEGER NOT NULL, window_end_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
		PRIMARY KEY(user_id, local_date, prayer), UNIQUE(user_id, external_id)
	);`,
	`CREATE INDEX IF NOT EXISTS idx_prayer_occurrences_user_time ON prayer_occurrences(user_id, scheduled_at, window_end_at);`,
}

func Migrations() []string { return migrations }

type Settings struct {
	Location          string    `json:"location"`
	LocationLabel     string    `json:"location_label,omitempty"`
	Latitude          float64   `json:"latitude,omitempty"`
	Longitude         float64   `json:"longitude,omitempty"`
	Timezone          string    `json:"timezone,omitempty"`
	Method            Method    `json:"method"`
	AsrSchool         AsrSchool `json:"asr_school"`
	TodoSync          bool      `json:"todo_sync"`
	TrackingStartedAt int64     `json:"tracking_started_at,omitempty"`
	SyncWarning       string    `json:"sync_warning,omitempty"`
	Configured        bool      `json:"configured"`
}

func defaultSettings() Settings { return Settings{Method: MethodMWL, AsrSchool: AsrStandard} }

func (a *App) withConn(ctx context.Context, fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return fmt.Errorf("prayer database is not configured")
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return fn(conn)
}

func (a *App) loadSettings(ctx context.Context, userID string) (Settings, error) {
	out := defaultSettings()
	err := a.withConn(ctx, func(conn *sqlite.Conn) error {
		_, err := preferences.GetJSON(conn, userID, sourceName, settingsKey, &out)
		return err
	})
	if out.Method == "" {
		out.Method = MethodMWL
	}
	if out.AsrSchool == "" {
		out.AsrSchool = AsrStandard
	}
	out.Configured = out.LocationLabel != "" && out.Timezone != ""
	return out, err
}

func (a *App) saveSettings(ctx context.Context, userID string, settings Settings) error {
	settings.Configured = settings.LocationLabel != "" && settings.Timezone != ""
	return a.withConn(ctx, func(conn *sqlite.Conn) error {
		return preferences.SetJSON(conn, userID, sourceName, settingsKey, settings)
	})
}

func (a *App) enabledUsers(ctx context.Context) []string {
	var users []string
	_ = a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT user_id,value_json FROM user_preferences WHERE app=? AND key=?`, &sqlitex.ExecOptions{Args: []any{sourceName, settingsKey}, ResultFunc: func(stmt *sqlite.Stmt) error {
			var settings Settings
			if json.Unmarshal([]byte(stmt.ColumnText(1)), &settings) == nil && settings.TodoSync && settings.LocationLabel != "" {
				users = append(users, stmt.ColumnText(0))
			}
			return nil
		}})
	})
	return users
}

type occurrence struct {
	LocalDate  string
	Prayer     string
	ExternalID string
	Scheduled  int64
	WindowEnd  int64
}

func (a *App) upsertOccurrences(ctx context.Context, userID string, values []occurrence) error {
	return a.withConn(ctx, func(conn *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(conn)
		if err != nil {
			return err
		}
		defer end(&err)
		now := a.now().Unix()
		for _, value := range values {
			if err := sqlitex.Execute(conn, `INSERT INTO prayer_occurrences(user_id,local_date,prayer,external_id,scheduled_at,window_end_at,created_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(user_id,local_date,prayer) DO UPDATE SET external_id=excluded.external_id,scheduled_at=excluded.scheduled_at,window_end_at=excluded.window_end_at WHERE prayer_occurrences.scheduled_at>=?`, &sqlitex.ExecOptions{Args: []any{userID, value.LocalDate, value.Prayer, value.ExternalID, value.Scheduled, value.WindowEnd, now, now}}); err != nil {
				return err
			}
		}
		return nil
	})
}

func (a *App) occurrences(ctx context.Context, userID string) ([]occurrence, error) {
	var out []occurrence
	err := a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT local_date,prayer,external_id,scheduled_at,window_end_at FROM prayer_occurrences WHERE user_id=? ORDER BY scheduled_at`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			out = append(out, occurrence{LocalDate: stmt.ColumnText(0), Prayer: stmt.ColumnText(1), ExternalID: stmt.ColumnText(2), Scheduled: stmt.ColumnInt64(3), WindowEnd: stmt.ColumnInt64(4)})
			return nil
		}})
	})
	return out, err
}

func (a *App) removeOccurrenceIDs(ctx context.Context, userID string, externalIDs map[string]bool) error {
	if len(externalIDs) == 0 {
		return nil
	}
	return a.withConn(ctx, func(conn *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(conn)
		if err != nil {
			return err
		}
		defer end(&err)
		for id := range externalIDs {
			if err := sqlitex.Execute(conn, `DELETE FROM prayer_occurrences WHERE user_id=? AND external_id=?`, &sqlitex.ExecOptions{Args: []any{userID, id}}); err != nil {
				return err
			}
		}
		return nil
	})
}

func (a *App) setSyncWarning(ctx context.Context, userID, warning string) {
	settings, err := a.loadSettings(ctx, userID)
	if err != nil {
		return
	}
	settings.SyncWarning = warning
	_ = a.saveSettings(ctx, userID, settings)
}
