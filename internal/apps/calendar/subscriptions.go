package calendar

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
	"time"

	"github.com/emersion/go-ical"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

// handleSubscriptions manages read-only WebCal/ICS calendars. Remote events
// are stored in the same event table as local ones, so every view and CalDAV
// query has one source of truth.
func (a *App) handleSubscriptions(w http.ResponseWriter, r *http.Request) {
	uid := a.userID(r)
	if a.db == nil {
		http.Error(w, "calendar database unavailable", 503)
		return
	}
	switch r.Method {
	case http.MethodGet:
		var out []CalendarMeta
		_ = a.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `SELECT id,name,color,visible,sort_order,subscription_url,last_synced_at,last_sync_error FROM calendars WHERE user_id=? AND kind='subscription' ORDER BY sort_order,name`, &sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(s *sqlite.Stmt) error {
				out = append(out, CalendarMeta{ID: s.ColumnText(0), Name: s.ColumnText(1), Color: s.ColumnText(2), Visible: s.ColumnInt64(3) != 0, Order: int(s.ColumnInt64(4)), Kind: "subscription", SubscriptionURL: s.ColumnText(5), ReadOnly: true, LastSyncedAt: s.ColumnInt64(6), LastSyncError: s.ColumnText(7)})
				return nil
			}})
		})
		a.writeJSON(w, map[string]any{"calendars": out})
	case http.MethodPost:
		var in struct {
			Name  string `json:"name"`
			URL   string `json:"url"`
			Color string `json:"color"`
		}
		if json.NewDecoder(r.Body).Decode(&in) != nil || strings.TrimSpace(in.URL) == "" {
			http.Error(w, "name and url required", 400)
			return
		}
		url := strings.TrimSpace(in.URL)
		if strings.HasPrefix(url, "webcal://") {
			url = "https://" + strings.TrimPrefix(url, "webcal://")
		}
		if !strings.HasPrefix(url, "https://") && !strings.HasPrefix(url, "http://") {
			http.Error(w, "subscription URL must be http(s) or webcal", 400)
			return
		}
		if _, err := a.outbound.Validate(r.Context(), url); err != nil {
			http.Error(w, "subscription target is not allowed", http.StatusBadRequest)
			return
		}
		id := a.sanitizeCalID(in.Name)
		if id == "" {
			randomID, err := newCalendarID()
			if err != nil {
				http.Error(w, "could not create subscription", http.StatusInternalServerError)
				return
			}
			id = "subscription-" + randomID
		}
		if in.Name == "" {
			in.Name = id
		}
		if in.Color == "" {
			in.Color = calendarTheme.Sky
		}
		err := a.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO calendars (user_id,id,name,color,visible,sort_order,kind,subscription_url,read_only) VALUES (?,?,?,?,1,0,'subscription',?,1)`, &sqlitex.ExecOptions{Args: []any{uid, id, in.Name, in.Color, url}})
		})
		if err != nil {
			http.Error(w, "subscription already exists", 409)
			return
		}
		a.refreshSubscription(r.Context(), uid, id)
		a.recordActivity(r.Context(), uid, "subscription_created", "subscribed to calendar "+in.Name, "calendar", id, in.Name, "api")
		a.writeJSON(w, CalendarMeta{ID: id, Name: in.Name, Color: in.Color, Visible: true, Kind: "subscription", SubscriptionURL: url, ReadOnly: true})
	case http.MethodDelete:
		id := a.sanitizeCalID(r.URL.Query().Get("id"))
		_ = a.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM calendars WHERE user_id=? AND id=? AND kind='subscription'`, &sqlitex.ExecOptions{Args: []any{uid, id}})
		})
		a.recordActivity(r.Context(), uid, "subscription_deleted", "removed calendar subscription "+id, "calendar", id, id, "api")
		a.writeJSON(w, map[string]string{"status": "ok"})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (a *App) handleSubscriptionRefresh(w http.ResponseWriter, r *http.Request) {
	id := a.sanitizeCalID(r.URL.Query().Get("id"))
	if id == "" {
		http.Error(w, "missing subscription id", 400)
		return
	}
	if err := a.refreshSubscription(r.Context(), a.userID(r), id); err != nil {
		log.Printf("refresh calendar subscription %q: %v", id, err)
		http.Error(w, "subscription refresh failed", http.StatusBadGateway)
		return
	}
	a.recordActivity(r.Context(), a.userID(r), "subscription_sync", "synchronized calendar subscription "+id, "calendar", id, id, "background")
	a.writeJSON(w, map[string]string{"status": "ok"})
}
func (a *App) refreshAllSubscriptions(ctx context.Context) {
	if a.db == nil {
		return
	}
	var pairs [][2]string
	_ = a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT user_id,id FROM calendars WHERE kind='subscription'`, &sqlitex.ExecOptions{ResultFunc: func(s *sqlite.Stmt) error {
			pairs = append(pairs, [2]string{s.ColumnText(0), s.ColumnText(1)})
			return nil
		}})
	})
	for _, p := range pairs {
		err := a.refreshSubscription(ctx, p[0], p[1])
		summary := "synchronized calendar subscription " + p[1]
		if err != nil {
			summary = "calendar subscription sync failed for " + p[1]
		}
		a.recordActivity(ctx, p[0], "subscription_sync", summary, "calendar", p[1], p[1], "background")
	}
}
func (a *App) refreshSubscription(ctx context.Context, uid, id string) error {
	var url string
	err := a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT subscription_url FROM calendars WHERE user_id=? AND id=? AND kind='subscription'`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error { url = s.ColumnText(0); return nil }})
	})
	if err != nil || url == "" {
		return fmt.Errorf("subscription not found")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	res, err := a.outbound.Client(20 * time.Second).Do(req)
	if err != nil {
		return a.recordSyncError(uid, id, err)
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return a.recordSyncError(uid, id, fmt.Errorf("remote calendar returned %s", res.Status))
	}
	data, err := io.ReadAll(io.LimitReader(res.Body, (10<<20)+1))
	if err != nil {
		return a.recordSyncError(uid, id, err)
	}
	if len(data) > 10<<20 {
		return a.recordSyncError(uid, id, fmt.Errorf("remote calendar exceeds 10 MiB"))
	}
	dec := ical.NewDecoder(strings.NewReader(string(data)))
	var events []Event
	for {
		cal, e := dec.Decode()
		if e == io.EOF {
			break
		}
		if e != nil {
			return a.recordSyncError(uid, id, e)
		}
		for _, component := range cal.Events() {
			if event, ok := a.subscriptionEvent(component, id, len(events)); ok {
				events = append(events, event)
			}
		}
	}
	// Only replace the old snapshot after the complete feed decoded successfully.
	// The replacement is one transaction, so readers never see a partial feed.
	if err := a.sqlReplaceCalendarEvents(uid, id, events); err != nil {
		return a.recordSyncError(uid, id, err)
	}
	return a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE calendars SET last_synced_at=?,last_sync_error='' WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{time.Now().Unix(), uid, id}})
	})
}

func (a *App) subscriptionEvent(ev ical.Event, calendarID string, index int) (Event, bool) {
	e := Event{CalendarID: calendarID, ID: fmt.Sprintf("remote-%d", index), Busy: true, Status: "CONFIRMED"}
	if p := ev.Props.Get(ical.PropUID); p != nil && strings.TrimSpace(p.Value) != "" {
		e.ID = p.Value
	}
	if p := ev.Props.Get(ical.PropSummary); p != nil {
		e.Title = p.Value
	}
	if p := ev.Props.Get(ical.PropDescription); p != nil {
		e.Description = p.Value
	}
	if p := ev.Props.Get(ical.PropLocation); p != nil {
		e.Location = p.Value
	}
	if p := ev.Props.Get(ical.PropStatus); p != nil {
		e.Status = p.Value
	}
	if p := ev.Props.Get(ical.PropTransparency); p != nil {
		e.Busy = !strings.EqualFold(p.Value, "TRANSPARENT")
	}
	if p := ev.Props.Get(ical.PropRecurrenceRule); p != nil {
		e.RRule = p.Value
	}
	if p := ev.Props.Get(ical.PropDateTimeStart); p != nil {
		e.Start = a.parseICSDate(p.Value)
		e.AllDay = len(p.Value) == 8
		e.Timezone = p.Params.Get("TZID")
	}
	if p := ev.Props.Get(ical.PropDateTimeEnd); p != nil {
		e.End = a.parseICSDate(p.Value)
	}
	if e.Start == 0 {
		return Event{}, false
	}
	if e.End == 0 {
		if e.AllDay {
			e.End = e.Start + 86400
		} else {
			e.End = e.Start + 3600
		}
	} else if !e.AllDay && e.End <= e.Start {
		// Brightspace and similar course feeds model deadlines as point-in-time
		// events (DTEND equals DTSTART). Give those events a minimal duration so
		// they survive range rendering and remain visible at their due time.
		e.End = e.Start + 60
	}
	if p := ev.Props.Get(ical.PropCategories); p != nil {
		if values, err := p.TextList(); err == nil {
			e.Categories = values
		}
	}
	for _, p := range ev.Props.Values(ical.PropExceptionDates) {
		for _, value := range strings.Split(p.Value, ",") {
			if ts := a.parseICSDate(strings.TrimSpace(value)); ts != 0 {
				e.ExDates = append(e.ExDates, ts)
			}
		}
	}
	for _, child := range ev.Children {
		if child.Name == ical.CompAlarm {
			if p := child.Props.Get(ical.PropTrigger); p != nil {
				if minutes, ok := a.parseAlarmTriggerMinutes(p.Value); ok {
					e.Reminders = append(e.Reminders, minutes)
				}
			}
		}
	}
	return e, true
}
func (a *App) recordSyncError(uid, id string, err error) error {
	_ = a.withConn(func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE calendars SET last_sync_error=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{err.Error(), uid, id}})
	})
	return err
}
