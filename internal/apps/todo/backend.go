// Package todo implements a SQLite-backed Nextcloud Tasks-compatible task app.
package todo

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var todoTheme = theme.MochaPink()

type TaskList struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Color      string `json:"color"`
	SortOrder  int    `json:"sort_order"`
	Permission string `json:"permission,omitempty"`
	DeletedAt  int64  `json:"deleted_at,omitempty"`
}

type Task struct {
	ID              string   `json:"id"`
	ListID          string   `json:"list_id"`
	ParentID        string   `json:"parent_id,omitempty"`
	Title           string   `json:"title"`
	Notes           string   `json:"notes,omitempty"`
	Status          string   `json:"status"`
	PercentComplete int      `json:"percent_complete"`
	Priority        int      `json:"priority"`
	Starred         bool     `json:"starred"`
	StartAt         int64    `json:"start_at,omitempty"`
	DueAt           int64    `json:"due_at,omitempty"`
	CompletedAt     int64    `json:"completed_at,omitempty"`
	RRule           string   `json:"rrule,omitempty"`
	IsHabit         bool     `json:"is_habit"`
	Tags            []string `json:"tags"`
	CreatedAt       int64    `json:"created_at"`
	UpdatedAt       int64    `json:"updated_at"`
	Streak          int      `json:"streak,omitempty"`
	BestStreak      int      `json:"best_streak,omitempty"`
}

type Preferences struct {
	DefaultListID    string            `json:"default_list_id"`
	SmartCollections map[string]string `json:"smart_collections"`
	SortMode         string            `json:"sort_mode"`
	CollectionCounts map[string]int    `json:"collection_counts,omitempty"`
}

var smartCollectionNames = []string{"important", "today", "week", "all", "current", "completed"}

func defaultPreferences() Preferences {
	modes := make(map[string]string, len(smartCollectionNames))
	for _, name := range smartCollectionNames {
		modes[name] = "automatic"
	}
	return Preferences{SmartCollections: modes, SortMode: "due"}
}

func normalizePreferences(p Preferences) (Preferences, error) {
	out := defaultPreferences()
	out.DefaultListID = strings.TrimSpace(p.DefaultListID)
	if p.SortMode != "" {
		out.SortMode = p.SortMode
	}
	if out.SortMode != "due" && out.SortMode != "priority" {
		return out, fmt.Errorf("sort_mode must be due or priority")
	}
	for name, mode := range p.SmartCollections {
		known := false
		for _, candidate := range smartCollectionNames {
			if name == candidate {
				known = true
				break
			}
		}
		if !known {
			return out, fmt.Errorf("unknown smart collection %q", name)
		}
		if mode != "hidden" && mode != "visible" && mode != "automatic" {
			return out, fmt.Errorf("smart collection %s must be hidden, visible, or automatic", name)
		}
		out.SmartCollections[name] = mode
	}
	return out, nil
}

func (a *App) userID(r *http.Request) string {
	if u := server.GetUser(r); u != nil {
		return u.ID
	}
	return ""
}

func taskID() string { var b [16]byte; _, _ = rand.Read(b[:]); return "t" + hex.EncodeToString(b[:]) }

func (a *App) ensureDefaultList(uid string) (TaskList, error) {
	var list TaskList
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,name,color,sort_order FROM todo_lists WHERE owner_id=? AND deleted_at=0 ORDER BY sort_order,name LIMIT 1`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			list = TaskList{ID: s.ColumnText(0), Name: s.ColumnText(1), Color: s.ColumnText(2), SortOrder: int(s.ColumnInt64(3)), Permission: "owner"}
			return nil
		}})
	})
	if err != nil || list.ID != "" {
		return list, err
	}
	now := time.Now().Unix()
	list = TaskList{ID: "tasks-" + uid, Name: "Tasks", Color: todoTheme.Pink, Permission: "owner"}
	return list, a.exec(`INSERT INTO todo_lists(id,owner_id,name,color,sort_order,deleted_at,created_at,updated_at) VALUES(?,?,?,?,0,0,?,?)`, list.ID, uid, list.Name, list.Color, now, now)
}

func (a *App) listPermission(uid, listID string) (string, error) {
	var permission string
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT CASE WHEN owner_id=? THEN 'owner' ELSE COALESCE((SELECT permission FROM todo_list_members WHERE list_id=todo_lists.id AND user_id=?),'') END FROM todo_lists WHERE id=? AND deleted_at=0`, &sqlitex.ExecOptions{Args: []any{uid, uid, listID}, ResultFunc: func(s *sqlite.Stmt) error { permission = s.ColumnText(0); return nil }})
	})
	return permission, err
}

func canWrite(p string) bool { return p == "owner" || p == "write" }

func (a *App) lists(uid string, includeTrash bool) ([]TaskList, error) {
	_, err := a.ensureDefaultList(uid)
	if err != nil {
		return nil, err
	}
	var out []TaskList
	err = a.withConn(func(c *sqlite.Conn) error {
		query := `SELECT l.id,l.name,l.color,l.sort_order,l.deleted_at,CASE WHEN l.owner_id=? THEN 'owner' ELSE m.permission END FROM todo_lists l LEFT JOIN todo_list_members m ON m.list_id=l.id AND m.user_id=? WHERE (l.owner_id=? OR m.user_id=?)`
		if includeTrash {
			query += ` AND l.deleted_at>0`
		} else {
			query += ` AND l.deleted_at=0`
		}
		query += ` ORDER BY l.sort_order,l.name`
		return sqlitex.Execute(c, query, &sqlitex.ExecOptions{Args: []any{uid, uid, uid, uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, TaskList{ID: s.ColumnText(0), Name: s.ColumnText(1), Color: s.ColumnText(2), SortOrder: int(s.ColumnInt64(3)), DeletedAt: s.ColumnInt64(4), Permission: s.ColumnText(5)})
			return nil
		}})
	})
	return out, err
}

func scanTask(s *sqlite.Stmt) Task {
	t := Task{ID: s.ColumnText(0), ListID: s.ColumnText(1), ParentID: s.ColumnText(2), Title: s.ColumnText(3), Notes: s.ColumnText(4), Status: s.ColumnText(5), PercentComplete: int(s.ColumnInt64(6)), Priority: int(s.ColumnInt64(7)), Starred: s.ColumnInt64(8) != 0, StartAt: s.ColumnInt64(9), DueAt: s.ColumnInt64(10), CompletedAt: s.ColumnInt64(11), RRule: s.ColumnText(12), IsHabit: s.ColumnInt64(13) != 0, CreatedAt: s.ColumnInt64(15), UpdatedAt: s.ColumnInt64(16)}
	_ = json.Unmarshal([]byte(s.ColumnText(14)), &t.Tags)
	if t.Tags == nil {
		t.Tags = []string{}
	}
	return t
}

const taskColumns = `id,list_id,parent_id,title,notes,status,percent_complete,priority,starred,start_at,due_at,completed_at,rrule,is_habit,tags_json,created_at,updated_at`

func (a *App) tasks(uid, listID, collection string, includeTrash bool) ([]Task, error) {
	var out []Task
	err := a.withConn(func(c *sqlite.Conn) error {
		q := `SELECT ` + taskColumns + ` FROM todo_tasks t WHERE t.deleted_at` + map[bool]string{true: `>0`, false: `=0`}[includeTrash]
		args := []any{}
		if listID != "" {
			q += ` AND t.list_id=?`
			args = append(args, listID)
		}
		switch collection {
		case "important":
			q += ` AND t.starred=1`
		case "today":
			q += ` AND t.due_at BETWEEN ? AND ?`
			start := time.Now().Truncate(24 * time.Hour).Unix()
			args = append(args, start, start+86400)
		case "week":
			q += ` AND t.due_at BETWEEN ? AND ?`
			start := time.Now().Truncate(24 * time.Hour).Unix()
			args = append(args, start, start+7*86400)
		case "completed":
			q += ` AND t.status='COMPLETED'`
		case "current":
			q += ` AND t.status!='COMPLETED' AND t.start_at>0 AND t.start_at<=?`
			args = append(args, time.Now().Unix())
		case "all":
		}
		q += ` AND EXISTS(SELECT 1 FROM todo_lists l LEFT JOIN todo_list_members m ON m.list_id=l.id AND m.user_id=? WHERE l.id=t.list_id AND (l.owner_id=? OR m.user_id=?)) ORDER BY CASE WHEN due_at=0 THEN 1 ELSE 0 END,due_at,priority DESC,updated_at DESC`
		args = append(args, uid, uid, uid)
		return sqlitex.Execute(c, q, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error { out = append(out, scanTask(s)); return nil }})
	})
	if err != nil {
		return nil, err
	}
	for i := range out {
		if out[i].IsHabit {
			out[i].Streak, out[i].BestStreak = a.streak(out[i].ID)
		}
	}
	return out, nil
}

func (a *App) streak(id string) (int, int) {
	var completed []int64
	_ = a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT completed_at FROM todo_occurrences WHERE task_id=? AND completed_at>0 ORDER BY scheduled_at`, &sqlitex.ExecOptions{Args: []any{id}, ResultFunc: func(s *sqlite.Stmt) error { completed = append(completed, s.ColumnInt64(0)); return nil }})
	})
	best, current := 0, 0
	for range completed {
		current++
		if current > best {
			best = current
		}
	}
	return current, best
}

func validStatus(s string) bool {
	return s == "NEEDS-ACTION" || s == "IN-PROCESS" || s == "COMPLETED" || s == "CANCELLED"
}
func normalizeTask(t *Task) error {
	t.Title = strings.TrimSpace(t.Title)
	if t.Title == "" {
		return fmt.Errorf("title is required")
	}
	if len(t.Title) > 512 {
		return fmt.Errorf("title is too long")
	}
	if t.Status == "" {
		t.Status = "NEEDS-ACTION"
	}
	if !validStatus(t.Status) {
		return fmt.Errorf("invalid status")
	}
	if t.PercentComplete < 0 || t.PercentComplete > 100 {
		return fmt.Errorf("percent_complete must be 0..100")
	}
	if t.Priority < 0 || t.Priority > 9 {
		return fmt.Errorf("priority must be 0..9")
	}
	if t.RRule != "" {
		if _, err := nextDue(t.RRule, max(t.DueAt, time.Now().Unix())); err != nil {
			return err
		}
	}
	return nil
}
func max(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}

func nextDue(rule string, from int64) (int64, error) {
	parts := map[string]string{}
	for _, p := range strings.Split(strings.ToUpper(rule), ";") {
		kv := strings.SplitN(p, "=", 2)
		if len(kv) == 2 {
			parts[kv[0]] = kv[1]
		}
	}
	freq := parts["FREQ"]
	interval := 1
	if raw := parts["INTERVAL"]; raw != "" {
		v, e := strconv.Atoi(raw)
		if e != nil || v < 1 {
			return 0, fmt.Errorf("invalid recurrence interval")
		}
		interval = v
	}
	t := time.Unix(from, 0)
	switch freq {
	case "DAILY":
		return t.AddDate(0, 0, interval).Unix(), nil
	case "WEEKLY":
		return t.AddDate(0, 0, 7*interval).Unix(), nil
	case "MONTHLY":
		return t.AddDate(0, interval, 0).Unix(), nil
	case "YEARLY":
		return t.AddDate(interval, 0, 0).Unix(), nil
	default:
		return 0, fmt.Errorf("rrule must define DAILY, WEEKLY, MONTHLY, or YEARLY FREQ")
	}
}

func (a *App) saveTask(t Task) error {
	tags, _ := json.Marshal(t.Tags)
	return a.exec(`UPDATE todo_tasks SET title=?,notes=?,status=?,percent_complete=?,priority=?,starred=?,start_at=?,due_at=?,completed_at=?,rrule=?,is_habit=?,tags_json=?,parent_id=?,updated_at=? WHERE id=?`, t.Title, t.Notes, t.Status, t.PercentComplete, t.Priority, boolInt(t.Starred), t.StartAt, t.DueAt, t.CompletedAt, t.RRule, boolInt(t.IsHabit), string(tags), t.ParentID, time.Now().Unix(), t.ID)
}

func (a *App) getTask(uid, id string) (Task, string, error) {
	var t Task
	p, err := a.listPermissionForTask(uid, id, &t)
	return t, p, err
}
func (a *App) listPermissionForTask(uid, id string, out *Task) (string, error) {
	var p string
	columns := "t." + strings.ReplaceAll(taskColumns, ",", ",t.")
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT `+columns+`,CASE WHEN l.owner_id=? THEN 'owner' ELSE COALESCE(m.permission,'') END FROM todo_tasks t JOIN todo_lists l ON l.id=t.list_id LEFT JOIN todo_list_members m ON m.list_id=l.id AND m.user_id=? WHERE t.id=?`, &sqlitex.ExecOptions{Args: []any{uid, uid, id}, ResultFunc: func(s *sqlite.Stmt) error { // task columns plus permission
			*out = scanTask(s)
			p = s.ColumnText(17)
			return nil
		}})
	})
	return p, err
}

func (a *App) handleLists(w http.ResponseWriter, r *http.Request) {
	uid := a.userID(r)
	switch r.Method {
	case http.MethodGet:
		v, err := a.lists(uid, r.URL.Query().Get("trash") == "1")
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		a.writeJSON(w, map[string]any{"lists": v})
	case http.MethodPost:
		var v TaskList
		if !a.decode(w, r, &v) {
			return
		}
		v.Name = strings.TrimSpace(v.Name)
		if v.Name == "" {
			a.fail(w, 400, fmt.Errorf("name is required"))
			return
		}
		if v.Color == "" {
			v.Color = todoTheme.Pink
		}
		v.ID = taskID()
		now := time.Now().Unix()
		if err := a.exec(`INSERT INTO todo_lists(id,owner_id,name,color,sort_order,deleted_at,created_at,updated_at) VALUES(?,?,?,?,0,0,?,?)`, v.ID, uid, v.Name, v.Color, now, now); err != nil {
			a.fail(w, 500, err)
			return
		}
		v.Permission = "owner"
		a.recordActivity(r.Context(), uid, v.ID, "list_created", "created task list "+v.Name, "task_list", v.ID, v.Name)
		a.writeJSON(w, v)
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}

func (a *App) handleList(w http.ResponseWriter, r *http.Request) {
	parts := strings.Split(strings.Trim(strings.TrimPrefix(r.URL.Path, "/api/todo/lists/"), "/"), "/")
	if len(parts) == 0 || parts[0] == "" {
		a.fail(w, 404, fmt.Errorf("not found"))
		return
	}
	uid, id := a.userID(r), parts[0]
	if r.Method == http.MethodPost && len(parts) == 2 && parts[1] == "restore" {
		var ownerID string
		err := a.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `SELECT owner_id FROM todo_lists WHERE id=? AND deleted_at>0`, &sqlitex.ExecOptions{
				Args:       []any{id},
				ResultFunc: func(s *sqlite.Stmt) error { ownerID = s.ColumnText(0); return nil },
			})
		})
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		if ownerID == "" {
			a.fail(w, 404, fmt.Errorf("not found"))
			return
		}
		if ownerID != uid {
			a.fail(w, 403, fmt.Errorf("owner permission required"))
			return
		}
		if err := a.exec(`UPDATE todo_lists SET deleted_at=0,updated_at=? WHERE id=?`, time.Now().Unix(), id); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, id, "list_restored", "restored task list "+id, "task_list", id, id)
		w.WriteHeader(http.StatusNoContent)
		return
	}
	p, err := a.listPermission(uid, id)
	if err != nil {
		a.fail(w, 500, err)
		return
	}
	if p == "" {
		a.fail(w, 404, fmt.Errorf("not found"))
		return
	}
	if len(parts) == 2 && parts[1] == "share" {
		a.handleShare(w, r, id, p)
		return
	}
	switch r.Method {
	case http.MethodPatch:
		if !canWrite(p) {
			a.fail(w, 403, fmt.Errorf("write permission required"))
			return
		}
		var in TaskList
		if !a.decode(w, r, &in) {
			return
		}
		if in.Name != "" {
			if err := a.exec(`UPDATE todo_lists SET name=?,color=CASE WHEN ?='' THEN color ELSE ? END,updated_at=? WHERE id=?`, strings.TrimSpace(in.Name), in.Color, in.Color, time.Now().Unix(), id); err != nil {
				a.fail(w, 500, err)
				return
			}
		}
		a.recordActivity(r.Context(), uid, id, "list_updated", "updated task list "+id, "task_list", id, id)
		w.WriteHeader(http.StatusNoContent)
	case http.MethodDelete:
		if p != "owner" {
			a.fail(w, 403, fmt.Errorf("owner permission required"))
			return
		}
		if err := a.exec(`UPDATE todo_lists SET deleted_at=?,updated_at=? WHERE id=?`, time.Now().Unix(), time.Now().Unix(), id); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, id, "list_deleted", "deleted task list "+id, "task_list", id, id)
		w.WriteHeader(http.StatusNoContent)
	case http.MethodPost:
		fallthrough
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}

func (a *App) handleShare(w http.ResponseWriter, r *http.Request, id, p string) {
	if p != "owner" {
		a.fail(w, 403, fmt.Errorf("owner permission required"))
		return
	}
	switch r.Method {
	case http.MethodGet:
		var v []map[string]string
		err := a.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `SELECT m.user_id,u.email,m.permission FROM todo_list_members m LEFT JOIN users u ON u.id=m.user_id WHERE m.list_id=? ORDER BY u.email`, &sqlitex.ExecOptions{Args: []any{id}, ResultFunc: func(s *sqlite.Stmt) error {
				v = append(v, map[string]string{"user_id": s.ColumnText(0), "email": s.ColumnText(1), "permission": s.ColumnText(2)})
				return nil
			}})
		})
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		a.writeJSON(w, map[string]any{"shares": v})
	case http.MethodPost:
		var in struct {
			Email      string `json:"email"`
			Permission string `json:"permission"`
		}
		if !a.decode(w, r, &in) {
			return
		}
		if in.Permission != "read" && in.Permission != "write" {
			a.fail(w, 400, fmt.Errorf("permission must be read or write"))
			return
		}
		u, err := a.userByEmail(in.Email)
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		if u == nil {
			a.fail(w, 400, fmt.Errorf("user is not registered"))
			return
		}
		if err := a.exec(`INSERT INTO todo_list_members(list_id,user_id,permission,created_at) VALUES(?,?,?,?) ON CONFLICT(list_id,user_id) DO UPDATE SET permission=excluded.permission`, id, u.ID, in.Permission, time.Now().Unix()); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), a.userID(r), id, "list_shared", "shared task list with "+u.Email, "task_list", id, id)
		a.writeJSON(w, map[string]string{"user_id": u.ID, "email": u.Email, "permission": in.Permission})
	case http.MethodDelete:
		userID := r.URL.Query().Get("user_id")
		if userID == "" {
			a.fail(w, 400, fmt.Errorf("user_id is required"))
			return
		}
		if err := a.exec(`DELETE FROM todo_list_members WHERE list_id=? AND user_id=?`, id, userID); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), a.userID(r), id, "list_unshared", "removed a task list share", "task_list", id, id)
		w.WriteHeader(http.StatusNoContent)
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}

func (a *App) userByEmail(email string) (*auth.User, error) {
	var u *auth.User
	err := a.withConn(func(c *sqlite.Conn) error { var err error; u, err = auth.GetUserByEmail(c, email); return err })
	return u, err
}

func (a *App) handleTasks(w http.ResponseWriter, r *http.Request) {
	uid := a.userID(r)
	switch r.Method {
	case http.MethodGet:
		v, err := a.tasks(uid, r.URL.Query().Get("list_id"), r.URL.Query().Get("collection"), r.URL.Query().Get("trash") == "1")
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		a.writeJSON(w, map[string]any{"tasks": v})
	case http.MethodPost:
		var t Task
		if !a.decode(w, r, &t) {
			return
		}
		if t.ListID == "" {
			l, err := a.ensureDefaultList(uid)
			if err != nil {
				a.fail(w, 500, err)
				return
			}
			t.ListID = l.ID
		}
		p, err := a.listPermission(uid, t.ListID)
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		if !canWrite(p) {
			a.fail(w, 403, fmt.Errorf("write permission required"))
			return
		}
		if err := normalizeTask(&t); err != nil {
			a.fail(w, 400, err)
			return
		}
		t.ID = taskID()
		now := time.Now().Unix()
		t.CreatedAt, t.UpdatedAt = now, now
		tags, _ := json.Marshal(t.Tags)
		if err := a.exec(`INSERT INTO todo_tasks(`+taskColumns+`,deleted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0)`, t.ID, t.ListID, t.ParentID, t.Title, t.Notes, t.Status, t.PercentComplete, t.Priority, boolInt(t.Starred), t.StartAt, t.DueAt, t.CompletedAt, t.RRule, boolInt(t.IsHabit), string(tags), now, now); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, t.ListID, "task_created", "created task "+t.Title, "task", t.ID, t.Title)
		a.writeJSON(w, t)
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}

func (a *App) handleTask(w http.ResponseWriter, r *http.Request) {
	id := strings.Trim(strings.TrimPrefix(r.URL.Path, "/api/todo/tasks/"), "/")
	if id == "" {
		a.fail(w, 404, fmt.Errorf("not found"))
		return
	}
	uid := a.userID(r)
	t, p, err := a.getTask(uid, id)
	if err != nil {
		a.fail(w, 500, err)
		return
	}
	if p == "" {
		a.fail(w, 404, fmt.Errorf("not found"))
		return
	}
	switch r.Method {
	case http.MethodGet:
		a.writeJSON(w, t)
	case http.MethodPatch:
		if !canWrite(p) {
			a.fail(w, 403, fmt.Errorf("write permission required"))
			return
		}
		var in Task
		if !a.decode(w, r, &in) {
			return
		}
		in.ID = t.ID
		in.ListID = t.ListID
		if in.Title == "" {
			in.Title = t.Title
		}
		if err := normalizeTask(&in); err != nil {
			a.fail(w, 400, err)
			return
		}
		if err := a.saveTask(in); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, in.ListID, "task_updated", "updated task "+in.Title, "task", in.ID, in.Title)
		a.writeJSON(w, in)
	case http.MethodPost:
		if !canWrite(p) {
			a.fail(w, 403, fmt.Errorf("write permission required"))
			return
		}
		action := r.URL.Query().Get("action")
		if action == "restore" {
			if err := a.exec(`UPDATE todo_tasks SET deleted_at=0,updated_at=? WHERE id=?`, time.Now().Unix(), id); err != nil {
				a.fail(w, 500, err)
				return
			}
			a.recordActivity(r.Context(), uid, t.ListID, "task_restored", "restored task "+t.Title, "task", t.ID, t.Title)
			w.WriteHeader(http.StatusNoContent)
			return
		}
		if action != "complete" {
			a.fail(w, 400, fmt.Errorf("unknown action"))
			return
		}
		a.complete(w, t, uid)
	case http.MethodDelete:
		if !canWrite(p) {
			a.fail(w, 403, fmt.Errorf("write permission required"))
			return
		}
		if err := a.exec(`UPDATE todo_tasks SET deleted_at=?,updated_at=? WHERE id=?`, time.Now().Unix(), time.Now().Unix(), id); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, t.ListID, "task_deleted", "deleted task "+t.Title, "task", t.ID, t.Title)
		w.WriteHeader(http.StatusNoContent)
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}

func (a *App) complete(w http.ResponseWriter, t Task, userID string) {
	now := time.Now().Unix()
	scheduled := t.DueAt
	if scheduled == 0 {
		scheduled = now
	}
	if t.RRule != "" {
		next, err := nextDue(t.RRule, scheduled)
		if err != nil {
			a.fail(w, 400, err)
			return
		}
		t.DueAt = next
		t.Status = "NEEDS-ACTION"
		t.PercentComplete = 0
		t.CompletedAt = 0
	} else {
		t.Status = "COMPLETED"
		t.PercentComplete = 100
		t.CompletedAt = now
	}
	tags, _ := json.Marshal(t.Tags)
	err := a.withConn(func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `INSERT INTO todo_occurrences(id,task_id,scheduled_at,completed_at,skipped,created_at) VALUES(?,?,?,?,0,?)`, &sqlitex.ExecOptions{Args: []any{taskID(), t.ID, scheduled, now, now}}); err != nil {
			return err
		}
		return sqlitex.Execute(c, `UPDATE todo_tasks SET title=?,notes=?,status=?,percent_complete=?,priority=?,starred=?,start_at=?,due_at=?,completed_at=?,rrule=?,is_habit=?,tags_json=?,parent_id=?,updated_at=? WHERE id=?`, &sqlitex.ExecOptions{Args: []any{t.Title, t.Notes, t.Status, t.PercentComplete, t.Priority, boolInt(t.Starred), t.StartAt, t.DueAt, t.CompletedAt, t.RRule, boolInt(t.IsHabit), string(tags), t.ParentID, now, t.ID}})
	})
	if err != nil {
		a.fail(w, 500, err)
		return
	}
	if t.IsHabit {
		t.Streak, t.BestStreak = a.streak(t.ID)
	}
	a.recordActivity(context.Background(), userID, t.ListID, "task_completed", "completed task "+t.Title, "task", t.ID, t.Title)
	a.writeJSON(w, t)
}

func (a *App) handlePreferences(w http.ResponseWriter, r *http.Request) {
	uid := a.userID(r)
	switch r.Method {
	case http.MethodGet:
		p, err := a.preferences(uid)
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		p.CollectionCounts, err = a.collectionCounts(uid)
		if err != nil {
			a.fail(w, 500, err)
			return
		}
		a.writeJSON(w, p)
	case http.MethodPut:
		var p Preferences
		if !a.decode(w, r, &p) {
			return
		}
		p, err := normalizePreferences(p)
		if err != nil {
			a.fail(w, 400, err)
			return
		}
		if p.DefaultListID != "" {
			permission, err := a.listPermission(uid, p.DefaultListID)
			if err != nil {
				a.fail(w, 500, err)
				return
			}
			if permission == "" {
				a.fail(w, 400, fmt.Errorf("default_list_id is not an accessible list"))
				return
			}
		}
		js, _ := json.Marshal(p.SmartCollections)
		if err := a.exec(`INSERT INTO todo_preferences(user_id,default_list_id,smart_collections_json,sort_mode) VALUES(?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET default_list_id=excluded.default_list_id,smart_collections_json=excluded.smart_collections_json,sort_mode=excluded.sort_mode`, uid, p.DefaultListID, string(js), p.SortMode); err != nil {
			a.fail(w, 500, err)
			return
		}
		a.recordActivity(r.Context(), uid, p.DefaultListID, "preferences_updated", "updated Todo preferences", "preferences", uid, "Todo preferences")
		a.writeJSON(w, p)
	default:
		a.fail(w, 405, fmt.Errorf("method not allowed"))
	}
}
func (a *App) preferences(uid string) (Preferences, error) {
	p := defaultPreferences()
	var defaultListID, rawModes, sortMode string
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT default_list_id,smart_collections_json,sort_mode FROM todo_preferences WHERE user_id=?`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			defaultListID = s.ColumnText(0)
			rawModes = s.ColumnText(1)
			sortMode = s.ColumnText(2)
			return nil
		}})
	})
	if err != nil {
		return p, err
	}
	if defaultListID == "" && rawModes == "" && sortMode == "" {
		return p, nil
	}
	var modes map[string]string
	if json.Unmarshal([]byte(rawModes), &modes) == nil {
		p.SmartCollections = modes
	}
	p.DefaultListID = defaultListID
	p.SortMode = sortMode
	normalized, normalizeErr := normalizePreferences(p)
	if normalizeErr != nil {
		return defaultPreferences(), nil
	}
	return normalized, nil
}

func (a *App) collectionCounts(uid string) (map[string]int, error) {
	counts := make(map[string]int, len(smartCollectionNames))
	start := time.Now().Truncate(24 * time.Hour).Unix()
	now := time.Now().Unix()
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT
			COALESCE(SUM(t.starred=1),0),
			COALESCE(SUM(t.due_at BETWEEN ? AND ?),0),
			COALESCE(SUM(t.due_at BETWEEN ? AND ?),0),
			COUNT(*),
			COALESCE(SUM(t.status!='COMPLETED' AND t.start_at>0 AND t.start_at<=?),0),
			COALESCE(SUM(t.status='COMPLETED'),0)
		 FROM todo_tasks t WHERE t.deleted_at=0 AND EXISTS(
			SELECT 1 FROM todo_lists l LEFT JOIN todo_list_members m ON m.list_id=l.id AND m.user_id=?
			WHERE l.id=t.list_id AND l.deleted_at=0 AND (l.owner_id=? OR m.user_id=?))`, &sqlitex.ExecOptions{Args: []any{start, start + 86400, start, start + 7*86400, now, uid, uid, uid}, ResultFunc: func(s *sqlite.Stmt) error {
			for i, name := range smartCollectionNames {
				counts[name] = int(s.ColumnInt64(i))
			}
			return nil
		}})
	})
	return counts, err
}

func (a *App) decode(w http.ResponseWriter, r *http.Request, v any) bool {
	if err := json.NewDecoder(r.Body).Decode(v); err != nil {
		a.fail(w, 400, fmt.Errorf("invalid JSON"))
		return false
	}
	return true
}
func (a *App) fail(w http.ResponseWriter, status int, err error) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
}
func (a *App) writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

// Keep sort imported for the upcoming CalDAV task ordering helpers.
var _ = sort.Slice
