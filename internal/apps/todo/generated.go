package todo

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

// GeneratedTask is a one-off task managed by another Dar app. ExternalID is
// stable within that source and makes retries safe across process restarts.
type GeneratedTask struct {
	ExternalID string
	Title      string
	StartAt    int64
	DueAt      int64
	Tags       []string
}

type GeneratedTaskState struct {
	ExternalID  string
	TaskID      string
	Status      string
	CompletedAt int64
	Deleted     bool
}

// ReconcileGeneratedTasks creates missing tasks and updates future incomplete
// tasks without undoing completion or an explicit user deletion.
func (a *App) ReconcileGeneratedTasks(ctx context.Context, userID, source, listName string, desired []GeneratedTask) (map[string]GeneratedTaskState, error) {
	userID, source, listName = strings.TrimSpace(userID), strings.TrimSpace(source), strings.TrimSpace(listName)
	if userID == "" || source == "" || listName == "" {
		return nil, fmt.Errorf("user, source, and list name are required")
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	states := make(map[string]GeneratedTaskState, len(desired))
	err = func() (err error) {
		end, err := sqlitex.ImmediateTransaction(conn)
		if err != nil {
			return err
		}
		defer end(&err)
		listID, err := ensureGeneratedList(conn, userID, source, listName)
		if err != nil {
			return err
		}
		now := time.Now().Unix()
		for _, spec := range desired {
			spec.ExternalID = strings.TrimSpace(spec.ExternalID)
			spec.Title = strings.TrimSpace(spec.Title)
			if spec.ExternalID == "" || spec.Title == "" || spec.DueAt <= 0 {
				return fmt.Errorf("generated task id, title, and due time are required")
			}
			state, dueAt, found, err := generatedState(conn, userID, source, spec.ExternalID)
			if err != nil {
				return err
			}
			if found {
				states[spec.ExternalID] = state
				if state.Deleted || state.Status == "COMPLETED" || dueAt < now {
					continue
				}
				tags, _ := json.Marshal(spec.Tags)
				if err := sqlitex.Execute(conn, `UPDATE todo_tasks SET list_id=?,title=?,start_at=?,due_at=?,tags_json=?,updated_at=? WHERE id=?`, &sqlitex.ExecOptions{Args: []any{listID, spec.Title, spec.StartAt, spec.DueAt, string(tags), now, state.TaskID}}); err != nil {
					return err
				}
				continue
			}
			task := taskID()
			tags, _ := json.Marshal(spec.Tags)
			if err := sqlitex.Execute(conn, `INSERT INTO todo_tasks(`+taskColumns+`,deleted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0)`, &sqlitex.ExecOptions{Args: []any{
				task, listID, "", spec.Title, "", "NEEDS-ACTION", 0, 0, 0, spec.StartAt, spec.DueAt, 0, "", 0, string(tags), now, now,
			}}); err != nil {
				return err
			}
			if err := sqlitex.Execute(conn, `INSERT INTO todo_generated_tasks(user_id,source,external_id,task_id) VALUES(?,?,?,?)`, &sqlitex.ExecOptions{Args: []any{userID, source, spec.ExternalID, task}}); err != nil {
				return err
			}
			states[spec.ExternalID] = GeneratedTaskState{ExternalID: spec.ExternalID, TaskID: task, Status: "NEEDS-ACTION"}
		}
		return nil
	}()
	if err == nil {
		a.markSearchDirty()
	}
	return states, err
}

func (a *App) GeneratedTaskStates(ctx context.Context, userID, source string) (map[string]GeneratedTaskState, error) {
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	out := map[string]GeneratedTaskState{}
	err = sqlitex.Execute(conn, `SELECT g.external_id,t.id,t.status,t.completed_at,t.deleted_at FROM todo_generated_tasks g JOIN todo_tasks t ON t.id=g.task_id JOIN todo_lists l ON l.id=t.list_id WHERE g.user_id=? AND g.source=? AND l.owner_id=?`, &sqlitex.ExecOptions{
		Args: []any{userID, source, userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			state := GeneratedTaskState{ExternalID: stmt.ColumnText(0), TaskID: stmt.ColumnText(1), Status: stmt.ColumnText(2), CompletedAt: stmt.ColumnInt64(3), Deleted: stmt.ColumnInt64(4) != 0}
			out[state.ExternalID] = state
			return nil
		},
	})
	return out, err
}

// RemoveGeneratedTasks removes only incomplete generated tasks and forgets
// their external IDs so a later explicit re-enable can create fresh tasks.
func (a *App) RemoveGeneratedTasks(ctx context.Context, userID, source string, externalIDs []string) ([]string, error) {
	if len(externalIDs) == 0 {
		return nil, nil
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	removed := make([]string, 0, len(externalIDs))
	err = func() (err error) {
		end, err := sqlitex.ImmediateTransaction(conn)
		if err != nil {
			return err
		}
		defer end(&err)
		now := time.Now().Unix()
		for _, externalID := range externalIDs {
			var taskID, status string
			if err := sqlitex.Execute(conn, `SELECT t.id,t.status FROM todo_generated_tasks g JOIN todo_tasks t ON t.id=g.task_id WHERE g.user_id=? AND g.source=? AND g.external_id=?`, &sqlitex.ExecOptions{Args: []any{userID, source, externalID}, ResultFunc: func(stmt *sqlite.Stmt) error { taskID, status = stmt.ColumnText(0), stmt.ColumnText(1); return nil }}); err != nil {
				return err
			}
			if taskID == "" || status == "COMPLETED" {
				continue
			}
			if err := sqlitex.Execute(conn, `UPDATE todo_tasks SET deleted_at=?,updated_at=? WHERE id=? AND status!='COMPLETED'`, &sqlitex.ExecOptions{Args: []any{now, now, taskID}}); err != nil {
				return err
			}
			if err := sqlitex.Execute(conn, `DELETE FROM todo_generated_tasks WHERE user_id=? AND source=? AND external_id=?`, &sqlitex.ExecOptions{Args: []any{userID, source, externalID}}); err != nil {
				return err
			}
			removed = append(removed, externalID)
		}
		return nil
	}()
	if err == nil {
		a.markSearchDirty()
	}
	return removed, err
}

func ensureGeneratedList(conn *sqlite.Conn, userID, source, name string) (string, error) {
	var listID string
	err := sqlitex.Execute(conn, `SELECT l.id FROM todo_generated_lists g JOIN todo_lists l ON l.id=g.list_id WHERE g.user_id=? AND g.source=? AND l.owner_id=? AND l.deleted_at=0`, &sqlitex.ExecOptions{Args: []any{userID, source, userID}, ResultFunc: func(stmt *sqlite.Stmt) error { listID = stmt.ColumnText(0); return nil }})
	if err != nil || listID != "" {
		return listID, err
	}
	listID = "list-" + taskID()
	now := time.Now().Unix()
	if err := sqlitex.Execute(conn, `INSERT INTO todo_lists(id,owner_id,name,color,sort_order,deleted_at,created_at,updated_at) VALUES(?,?,?,?,0,0,?,?)`, &sqlitex.ExecOptions{Args: []any{listID, userID, name, todoTheme.Pink, now, now}}); err != nil {
		return "", err
	}
	if err := sqlitex.Execute(conn, `INSERT INTO todo_generated_lists(user_id,source,list_id) VALUES(?,?,?) ON CONFLICT(user_id,source) DO UPDATE SET list_id=excluded.list_id`, &sqlitex.ExecOptions{Args: []any{userID, source, listID}}); err != nil {
		return "", err
	}
	return listID, nil
}

func generatedState(conn *sqlite.Conn, userID, source, externalID string) (GeneratedTaskState, int64, bool, error) {
	var state GeneratedTaskState
	var dueAt int64
	found := false
	err := sqlitex.Execute(conn, `SELECT t.id,t.status,t.completed_at,t.deleted_at,t.due_at FROM todo_generated_tasks g JOIN todo_tasks t ON t.id=g.task_id JOIN todo_lists l ON l.id=t.list_id WHERE g.user_id=? AND g.source=? AND g.external_id=? AND l.owner_id=?`, &sqlitex.ExecOptions{
		Args: []any{userID, source, externalID, userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			found = true
			state = GeneratedTaskState{ExternalID: externalID, TaskID: stmt.ColumnText(0), Status: stmt.ColumnText(1), CompletedAt: stmt.ColumnInt64(2), Deleted: stmt.ColumnInt64(3) != 0}
			dueAt = stmt.ColumnInt64(4)
			return nil
		},
	})
	return state, dueAt, found, err
}
