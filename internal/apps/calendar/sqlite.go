package calendar

// SQLite-backed Calendar storage. Calendar deliberately stores event fields
// rather than an opaque ICS blob so the web UI, CalDAV and subscriptions use
// one authoritative representation.

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var migrations = []string{
	`CREATE TABLE IF NOT EXISTS calendars (
		user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, color TEXT NOT NULL,
		visible INTEGER NOT NULL DEFAULT 1, sort_order INTEGER NOT NULL DEFAULT 0,
		kind TEXT NOT NULL DEFAULT 'local', subscription_url TEXT NOT NULL DEFAULT '',
		read_only INTEGER NOT NULL DEFAULT 0, last_synced_at INTEGER NOT NULL DEFAULT 0,
		last_sync_error TEXT NOT NULL DEFAULT '', PRIMARY KEY(user_id, id)
	);`,
	`CREATE TABLE IF NOT EXISTS calendar_events (
		user_id TEXT NOT NULL, calendar_id TEXT NOT NULL, id TEXT NOT NULL,
		title TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', location TEXT NOT NULL DEFAULT '',
		start_at INTEGER NOT NULL, end_at INTEGER NOT NULL, all_day INTEGER NOT NULL DEFAULT 0,
		color TEXT NOT NULL DEFAULT '', timezone TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'CONFIRMED',
		busy INTEGER NOT NULL DEFAULT 1, rrule TEXT NOT NULL DEFAULT '', categories_json TEXT NOT NULL DEFAULT '[]',
		exdates_json TEXT NOT NULL DEFAULT '[]', reminders_json TEXT NOT NULL DEFAULT '[]',
		updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(user_id, calendar_id, id),
		FOREIGN KEY(user_id, calendar_id) REFERENCES calendars(user_id, id) ON DELETE CASCADE
	);`,
	`CREATE INDEX IF NOT EXISTS idx_calendar_events_range ON calendar_events(user_id, calendar_id, start_at, end_at);`,
	`CREATE INDEX IF NOT EXISTS idx_calendar_events_nonrec_overlap ON calendar_events(user_id, rrule, end_at, start_at);`,
}

func Migrations() []string { return migrations }

func (a *App) withConn(fn func(*sqlite.Conn) error) error {
	if a.db == nil {
		return fmt.Errorf("calendar database is not configured")
	}
	c, release, err := a.db.Conn(context.Background())
	if err != nil {
		return err
	}
	defer release()
	return fn(c)
}

func (a *App) ensureSQLDefault(uid string) {
	if a.db == nil || uid == "" {
		return
	}
	_ = a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c,
			`INSERT INTO calendars (user_id,id,name,color,visible,sort_order) VALUES (?,?,?,?,1,0) ON CONFLICT(user_id,id) DO NOTHING`,
			&sqlitex.ExecOptions{Args: []any{uid, "default", "Personal", calendarTheme.Pink}})
	})
}

func (a *App) sqlCalendars(uid string) []CalendarMeta {
	a.ensureSQLDefault(uid)
	var out []CalendarMeta
	_ = a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT id,name,color,visible,sort_order,kind,subscription_url,read_only,last_synced_at,last_sync_error FROM calendars WHERE user_id=? ORDER BY sort_order,name`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
			out = append(out, CalendarMeta{ID: s.ColumnText(0), Name: s.ColumnText(1), Color: s.ColumnText(2), Visible: s.ColumnInt64(3) != 0, Order: int(s.ColumnInt64(4)), Kind: s.ColumnText(5), SubscriptionURL: s.ColumnText(6), ReadOnly: s.ColumnInt64(7) != 0, LastSyncedAt: s.ColumnInt64(8), LastSyncError: s.ColumnText(9)})
			return nil
		}})
	})
	return out
}

func (a *App) sqlUpdateCalendar(uid string, m CalendarMeta) error {
	return a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE calendars SET name=?,color=?,visible=?,sort_order=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{m.Name, m.Color, boolInt(m.Visible), m.Order, uid, m.ID}})
	})
}

func (a *App) sqlSaveCalendar(uid string, m CalendarMeta) error {
	return a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c,
			`INSERT INTO calendars (user_id,id,name,color,visible,sort_order) VALUES (?,?,?,?,?,?) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name,color=excluded.color,visible=excluded.visible,sort_order=excluded.sort_order`,
			&sqlitex.ExecOptions{Args: []any{uid, m.ID, m.Name, m.Color, boolInt(m.Visible), m.Order}})
	})
}
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func (a *App) sqlEvents(uid, cal string) []Event {
	return a.sqlEventsInRange(uid, cal, 0, 0)
}

// sqlEventsInRange returns non-recurring events that overlap the requested
// interval, along with recurring masters. Recurring masters must be returned
// even when their DTSTART predates the interval so they can be expanded by the
// caller. A zero range preserves the unbounded behavior used by export/search.
func (a *App) sqlEventsInRange(uid, cal string, start, end int64) []Event {
	var out []Event
	_ = a.withConn(func(c *sqlite.Conn) error {
		selectEvents := `SELECT calendar_id,id,title,description,location,start_at,end_at,all_day,color,timezone,status,busy,rrule,categories_json,exdates_json,reminders_json FROM calendar_events WHERE user_id=?`
		q := selectEvents
		args := []any{uid}
		if cal != "" {
			q += ` AND calendar_id=?`
			args = append(args, cal)
		}
		if start != 0 || end != 0 {
			// Keep recurring masters regardless of DTSTART, then query ordinary
			// events by overlap. A UNION lets SQLite use the overlap index for
			// the latter instead of scanning all events for an OR expression.
			recurring := q + ` AND rrule<>''`
			overlapping := selectEvents
			overlapArgs := []any{uid}
			if cal != "" {
				overlapping += ` AND calendar_id=?`
				overlapArgs = append(overlapArgs, cal)
			}
			overlapping += ` AND rrule='' AND start_at<? AND end_at>?`
			q = recurring + ` UNION ALL ` + overlapping
			args = append(args, overlapArgs...)
			args = append(args, end, start)
		}
		q += ` ORDER BY start_at,id`
		return sqlitex.Execute(c, q, &sqlitex.ExecOptions{Args: args, ResultFunc: func(s *sqlite.Stmt) error {
			e := Event{CalendarID: s.ColumnText(0), ID: s.ColumnText(1), Title: s.ColumnText(2), Description: s.ColumnText(3), Location: s.ColumnText(4), Start: s.ColumnInt64(5), End: s.ColumnInt64(6), AllDay: s.ColumnInt64(7) != 0, Color: s.ColumnText(8), Timezone: s.ColumnText(9), Status: s.ColumnText(10), Busy: s.ColumnInt64(11) != 0, RRule: s.ColumnText(12)}
			_ = json.Unmarshal([]byte(s.ColumnText(13)), &e.Categories)
			_ = json.Unmarshal([]byte(s.ColumnText(14)), &e.ExDates)
			_ = json.Unmarshal([]byte(s.ColumnText(15)), &e.Reminders)
			out = append(out, e)
			return nil
		}})
	})
	return out
}

func (a *App) sqlWriteEvent(uid string, e Event) error {
	return a.withConn(func(c *sqlite.Conn) error {
		return a.sqlWriteEventConn(c, uid, e)
	})
}

func (a *App) sqlWriteEventConn(c *sqlite.Conn, uid string, e Event) error {
	cats, _ := json.Marshal(e.Categories)
	ex, _ := json.Marshal(e.ExDates)
	rem, _ := json.Marshal(e.Reminders)
	return sqlitex.Execute(c, `INSERT INTO calendar_events (user_id,calendar_id,id,title,description,location,start_at,end_at,all_day,color,timezone,status,busy,rrule,categories_json,exdates_json,reminders_json,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id,calendar_id,id) DO UPDATE SET title=excluded.title,description=excluded.description,location=excluded.location,start_at=excluded.start_at,end_at=excluded.end_at,all_day=excluded.all_day,color=excluded.color,timezone=excluded.timezone,status=excluded.status,busy=excluded.busy,rrule=excluded.rrule,categories_json=excluded.categories_json,exdates_json=excluded.exdates_json,reminders_json=excluded.reminders_json,updated_at=excluded.updated_at`, &sqlitex.ExecOptions{Args: []any{uid, e.CalendarID, e.ID, e.Title, e.Description, e.Location, e.Start, e.End, boolInt(e.AllDay), e.Color, e.Timezone, e.Status, boolInt(e.Busy), e.RRule, string(cats), string(ex), string(rem), time.Now().Unix()}})
}

// sqlReplaceCalendarEvents atomically replaces a calendar snapshot. Keeping the
// delete and all writes on one connection avoids exposing an empty calendar and
// prevents a large subscription refresh from monopolizing the pool.
func (a *App) sqlReplaceCalendarEvents(uid, calendarID string, events []Event) error {
	return a.withConn(func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		if err = sqlitex.Execute(c, `DELETE FROM calendar_events WHERE user_id=? AND calendar_id=?`, &sqlitex.ExecOptions{Args: []any{uid, calendarID}}); err != nil {
			return err
		}
		for _, event := range events {
			if err = a.sqlWriteEventConn(c, uid, event); err != nil {
				return err
			}
		}
		return nil
	})
}
func (a *App) sqlDeleteEvent(uid, cal, id string) error {
	return a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `DELETE FROM calendar_events WHERE user_id=? AND calendar_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, cal, id}})
	})
}
func (a *App) sqlFindEvent(uid, id string) (Event, string, error) {
	for _, e := range a.sqlEvents(uid, "") {
		if e.ID == id {
			return e, e.CalendarID, nil
		}
	}
	return Event{}, "", fmt.Errorf("not found")
}
func sortedEvents(v []Event) []Event {
	sort.Slice(v, func(i, j int) bool { return v[i].Start < v[j].Start })
	return v
}
