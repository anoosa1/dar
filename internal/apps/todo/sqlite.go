package todo

import (
	"context"
	"fmt"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS todo_lists (
		id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, name TEXT NOT NULL, color TEXT NOT NULL DEFAULT '` + todoTheme.Pink + `',
		sort_order INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
	);`,
	`CREATE INDEX IF NOT EXISTS idx_todo_lists_owner ON todo_lists(owner_id, deleted_at, sort_order);`,
	`CREATE TABLE IF NOT EXISTS todo_list_members (
		list_id TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE, user_id TEXT NOT NULL,
		permission TEXT NOT NULL CHECK(permission IN ('read','write')), created_at INTEGER NOT NULL,
		PRIMARY KEY(list_id, user_id)
	);`,
	`CREATE TABLE IF NOT EXISTS todo_tasks (
		id TEXT PRIMARY KEY, list_id TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE, parent_id TEXT NOT NULL DEFAULT '',
		title TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'NEEDS-ACTION', percent_complete INTEGER NOT NULL DEFAULT 0,
		priority INTEGER NOT NULL DEFAULT 0, starred INTEGER NOT NULL DEFAULT 0, start_at INTEGER NOT NULL DEFAULT 0, due_at INTEGER NOT NULL DEFAULT 0,
		completed_at INTEGER NOT NULL DEFAULT 0, rrule TEXT NOT NULL DEFAULT '', is_habit INTEGER NOT NULL DEFAULT 0,
		tags_json TEXT NOT NULL DEFAULT '[]', deleted_at INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
	);`,
	`CREATE INDEX IF NOT EXISTS idx_todo_tasks_list ON todo_tasks(list_id, deleted_at, due_at, updated_at);`,
	`CREATE TABLE IF NOT EXISTS todo_occurrences (
		id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES todo_tasks(id) ON DELETE CASCADE, scheduled_at INTEGER NOT NULL,
		completed_at INTEGER NOT NULL DEFAULT 0, skipped INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL
	);`,
	`CREATE INDEX IF NOT EXISTS idx_todo_occurrences_task ON todo_occurrences(task_id, scheduled_at);`,
	`CREATE TABLE IF NOT EXISTS todo_preferences (
		user_id TEXT PRIMARY KEY, default_list_id TEXT NOT NULL DEFAULT '', smart_collections_json TEXT NOT NULL DEFAULT '{}', sort_mode TEXT NOT NULL DEFAULT 'due'
	);`,
}

// generatedTaskMigrations are appended after the platform's established
// migration sequence. Keeping them separate prevents adding this capability
// from changing the index of migrations that existing Dar databases already
// recorded as applied.
var generatedTaskMigrations = []string{
	`CREATE TABLE IF NOT EXISTS todo_generated_lists (
		user_id TEXT NOT NULL, source TEXT NOT NULL, list_id TEXT NOT NULL REFERENCES todo_lists(id),
		PRIMARY KEY(user_id, source)
	);`,
	`CREATE TABLE IF NOT EXISTS todo_generated_tasks (
		user_id TEXT NOT NULL, source TEXT NOT NULL, external_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES todo_tasks(id) ON DELETE CASCADE,
		PRIMARY KEY(user_id, source, external_id), UNIQUE(task_id)
	);`,
	`CREATE INDEX IF NOT EXISTS idx_todo_generated_tasks_source ON todo_generated_tasks(user_id, source, task_id);`,
}

func Migrations() []string { return migrations }

func GeneratedTaskMigrations() []string { return generatedTaskMigrations }

func (a *App) withConn(fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return fmt.Errorf("todo database is not configured")
	}
	c, release, err := a.db.Conn(context.Background())
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}

func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func (a *App) exec(sql string, args ...any) error {
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, sql, &sqlitex.ExecOptions{Args: args})
	})
	if err == nil {
		a.markSearchDirty()
	}
	return err
}
