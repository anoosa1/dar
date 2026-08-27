package todo

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/caldav"
)

// Collections implements caldav.Store. Todo lists are CalDAV collections of
// VTODO components and remain backed by the same SQLite task records as the UI.
func (a *App) Collections(uid string) ([]caldav.Collection, error) {
	lists, err := a.lists(uid, false)
	if err != nil {
		return nil, err
	}
	out := make([]caldav.Collection, 0, len(lists))
	for _, l := range lists {
		out = append(out, caldav.Collection{ID: l.ID, Name: l.Name, Color: l.Color, Permission: l.Permission})
	}
	return out, nil
}

func (a *App) CreateCollection(uid, name string) (caldav.Collection, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		name = "Tasks"
	}
	l := TaskList{ID: taskID(), Name: name, Color: todoTheme.Pink, Permission: "owner"}
	now := time.Now().Unix()
	if err := a.exec(`INSERT INTO todo_lists(id,owner_id,name,color,sort_order,deleted_at,created_at,updated_at) VALUES(?,?,?,?,0,0,?,?)`, l.ID, uid, l.Name, l.Color, now, now); err != nil {
		return caldav.Collection{}, err
	}
	a.recordActivityOrigin(context.Background(), uid, l.ID, "list_created", "created task list "+l.Name, "task_list", l.ID, l.Name, "dav")
	return caldav.Collection{ID: l.ID, Name: l.Name, Color: l.Color, Permission: l.Permission}, nil
}

func (a *App) Objects(uid, listID string) ([]caldav.Object, error) {
	tasks, err := a.tasks(uid, listID, "all", false)
	if err != nil {
		return nil, err
	}
	out := make([]caldav.Object, 0, len(tasks))
	for _, t := range tasks {
		out = append(out, caldav.Object{Href: t.ID + ".ics", Data: taskICS(t), ETag: taskETag(t)})
	}
	return out, nil
}
func (a *App) Object(uid, listID, href string) (caldav.Object, error) {
	id := strings.TrimSuffix(href, ".ics")
	t, p, err := a.getTask(uid, id)
	if err != nil {
		return caldav.Object{}, err
	}
	if p == "" || t.ListID != listID {
		return caldav.Object{}, fmt.Errorf("not found")
	}
	live := false
	tasks, err := a.tasks(uid, listID, "all", false)
	if err != nil {
		return caldav.Object{}, err
	}
	for _, candidate := range tasks {
		if candidate.ID == id {
			live = true
			break
		}
	}
	if !live {
		return caldav.Object{}, fmt.Errorf("not found")
	}
	return caldav.Object{Href: href, Data: taskICS(t), ETag: taskETag(t)}, nil
}
func (a *App) PutObject(uid, listID, href, data string) (caldav.Object, error) {
	p, err := a.listPermission(uid, listID)
	if err != nil {
		return caldav.Object{}, err
	}
	if !canWrite(p) {
		return caldav.Object{}, fmt.Errorf("write permission required")
	}
	t, err := taskFromICS(data)
	if err != nil {
		return caldav.Object{}, err
	}
	t.ID = strings.TrimSuffix(href, ".ics")
	t.ListID = listID
	existing, oldPerm, err := a.getTask(uid, t.ID)
	if err != nil {
		return caldav.Object{}, err
	}
	now := time.Now().Unix()
	if oldPerm != "" {
		t.CreatedAt = existing.CreatedAt
		if t.ParentID == "" {
			t.ParentID = existing.ParentID
		}
		if err := a.saveTask(t); err != nil {
			return caldav.Object{}, err
		}
	} else {
		if err := normalizeTask(&t); err != nil {
			return caldav.Object{}, err
		}
		tags := `[]`
		if err := a.exec(`INSERT INTO todo_tasks(`+taskColumns+`,deleted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0)`, t.ID, t.ListID, t.ParentID, t.Title, t.Notes, t.Status, t.PercentComplete, t.Priority, boolInt(t.Starred), t.StartAt, t.DueAt, t.CompletedAt, t.RRule, boolInt(t.IsHabit), tags, now, now); err != nil {
			return caldav.Object{}, err
		}
		t.CreatedAt, t.UpdatedAt = now, now
	}
	action, summary := "task_created", "created task "+t.Title
	if oldPerm != "" {
		action, summary = "task_updated", "updated task "+t.Title
	}
	a.recordActivityOrigin(context.Background(), uid, listID, action, summary, "task", t.ID, t.Title, "dav")
	return caldav.Object{Href: href, Data: taskICS(t), ETag: taskETag(t)}, nil
}
func (a *App) DeleteObject(uid, listID, href string) error {
	t, p, err := a.getTask(uid, strings.TrimSuffix(href, ".ics"))
	if err != nil {
		return err
	}
	if p == "" || t.ListID != listID || !canWrite(p) {
		return fmt.Errorf("not found")
	}
	if err := a.exec(`UPDATE todo_tasks SET deleted_at=?,updated_at=? WHERE id=?`, time.Now().Unix(), time.Now().Unix(), t.ID); err != nil {
		return err
	}
	a.recordActivityOrigin(context.Background(), uid, listID, "task_deleted", "deleted task "+t.Title, "task", t.ID, t.Title, "dav")
	return nil
}

func taskETag(t Task) string { return fmt.Sprintf(`"%x"`, t.UpdatedAt) }
func icsEscape(s string) string {
	return strings.NewReplacer(`\\`, `\\\\`, "\n", `\\n`, ",", `\\,`, `;`, `\\;`).Replace(s)
}
func icsTime(v int64) string {
	if v == 0 {
		return ""
	}
	return time.Unix(v, 0).UTC().Format("20060102T150405Z")
}
func taskICS(t Task) string {
	var b strings.Builder
	b.WriteString("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Tasks//EN\r\nBEGIN:VTODO\r\n")
	b.WriteString("UID:" + icsEscape(t.ID) + "\r\nDTSTAMP:" + icsTime(t.UpdatedAt) + "\r\nSUMMARY:" + icsEscape(t.Title) + "\r\n")
	if t.Notes != "" {
		b.WriteString("DESCRIPTION:" + icsEscape(t.Notes) + "\r\n")
	}
	if t.StartAt > 0 {
		b.WriteString("DTSTART:" + icsTime(t.StartAt) + "\r\n")
	}
	if t.DueAt > 0 {
		b.WriteString("DUE:" + icsTime(t.DueAt) + "\r\n")
	}
	if t.CompletedAt > 0 {
		b.WriteString("COMPLETED:" + icsTime(t.CompletedAt) + "\r\n")
	}
	b.WriteString("STATUS:" + t.Status + "\r\nPERCENT-COMPLETE:" + strconv.Itoa(t.PercentComplete) + "\r\nPRIORITY:" + strconv.Itoa(t.Priority) + "\r\n")
	if t.RRule != "" {
		b.WriteString("RRULE:" + t.RRule + "\r\n")
	}
	if len(t.Tags) > 0 {
		b.WriteString("CATEGORIES:" + icsEscape(strings.Join(t.Tags, ",")) + "\r\n")
	}
	if t.IsHabit {
		b.WriteString("X-DAR-HABIT:TRUE\r\n")
	}
	b.WriteString("END:VTODO\r\nEND:VCALENDAR\r\n")
	return b.String()
}

func taskFromICS(data string) (Task, error) {
	t := Task{Status: "NEEDS-ACTION", Tags: []string{}}
	lines := strings.FieldsFunc(strings.ReplaceAll(data, "\r\n", "\n"), func(r rune) bool { return r == '\n' })
	found := false
	for _, line := range lines {
		line = strings.TrimSpace(line)
		kv := strings.SplitN(line, ":", 2)
		if len(kv) != 2 {
			continue
		}
		key := strings.ToUpper(strings.SplitN(kv[0], ";", 2)[0])
		v := strings.NewReplacer(`\\n`, "\n", `\\,`, ",", `\\;`, ";", `\\\\`, `\\`).Replace(kv[1])
		switch key {
		case "BEGIN":
			if v == "VTODO" {
				found = true
			}
		case "SUMMARY":
			t.Title = v
		case "DESCRIPTION":
			t.Notes = v
		case "STATUS":
			t.Status = strings.ToUpper(v)
		case "PERCENT-COMPLETE":
			t.PercentComplete, _ = strconv.Atoi(v)
		case "PRIORITY":
			t.Priority, _ = strconv.Atoi(v)
		case "DTSTART":
			t.StartAt = parseICSTime(v)
		case "DUE":
			t.DueAt = parseICSTime(v)
		case "COMPLETED":
			t.CompletedAt = parseICSTime(v)
		case "RRULE":
			t.RRule = strings.ToUpper(v)
		case "CATEGORIES":
			if v != "" {
				t.Tags = strings.Split(v, ",")
			}
		case "X-DAR-HABIT":
			t.IsHabit = strings.EqualFold(v, "TRUE") || v == "1"
		}
	}
	if !found {
		return Task{}, fmt.Errorf("calendar data has no VTODO")
	}
	if t.Status == "COMPLETED" && t.CompletedAt == 0 {
		t.CompletedAt = time.Now().Unix()
	}
	if err := normalizeTask(&t); err != nil {
		return Task{}, err
	}
	return t, nil
}
func parseICSTime(v string) int64 {
	for _, layout := range []string{"20060102T150405Z", "20060102T150405", "20060102"} {
		if x, e := time.Parse(layout, v); e == nil {
			return x.Unix()
		}
	}
	return 0
}

var _ caldav.Store = (*App)(nil)
