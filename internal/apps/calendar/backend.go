// Calendar app backend: iCal storage, REST API, and CalDAV.
package calendar

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"encoding/xml"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/anoosa/dar/internal/safefile"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"github.com/emersion/go-ical"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var calendarTheme = theme.MochaPink()

// Event is the REST JSON model for a calendar event.
type Event struct {
	ID          string   `json:"id"`
	CalendarID  string   `json:"calendar"`
	Title       string   `json:"title"`
	Description string   `json:"description"`
	Location    string   `json:"location,omitempty"`
	Start       int64    `json:"start"`
	End         int64    `json:"end"`
	AllDay      bool     `json:"all_day"`
	Color       string   `json:"color"`
	Timezone    string   `json:"timezone,omitempty"`
	Status      string   `json:"status,omitempty"` // CONFIRMED|TENTATIVE|CANCELLED
	Busy        bool     `json:"busy"`             // true = OPAQUE (busy)
	RRule       string   `json:"rrule,omitempty"`
	Categories  []string `json:"categories,omitempty"`
	// ExDates are excluded occurrence starts (unix) for a recurring master.
	ExDates []int64 `json:"exdates,omitempty"`
	// Reminders are minutes-before-start VALARM offsets (e.g. 15 = 15 min before).
	Reminders []int `json:"reminders,omitempty"`
	// Expansion / edit-scope fields (not always persisted on master ICS):
	MasterID     string `json:"master_id,omitempty"`
	RecurrenceID int64  `json:"recurrence_id,omitempty"`
	Scope        string `json:"scope,omitempty"` // this|future|all (request-only)
}

func newCalendarID() (string, error) {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", fmt.Errorf("generate calendar id: %w", err)
	}
	return hex.EncodeToString(raw[:]), nil
}

// CalendarMeta is persisted as meta.json inside each calendar directory.
type CalendarMeta struct {
	ID              string `json:"id"`
	Name            string `json:"name"`
	Color           string `json:"color"`
	Visible         bool   `json:"visible"`
	Order           int    `json:"order"`
	Kind            string `json:"kind,omitempty"`
	SubscriptionURL string `json:"subscription_url,omitempty"`
	ReadOnly        bool   `json:"read_only,omitempty"`
	LastSyncedAt    int64  `json:"last_synced_at,omitempty"`
	LastSyncError   string `json:"last_sync_error,omitempty"`
}

func (app *App) calendarsDir(uid string) string {
	d := filepath.Join(app.dataRoot, uid, "calendars")
	os.MkdirAll(d, 0700)
	return d
}

func (app *App) calendarDir(uid, cal string) string {
	cal = app.sanitizeCalID(cal)
	if cal == "" {
		cal = "default"
	}
	d := filepath.Join(app.calendarsDir(uid), cal)
	os.MkdirAll(d, 0700)
	return d
}

func (app *App) defaultCalendarDir(uid string) string {
	app.ensureDefaultCalendar(uid)
	return app.calendarDir(uid, "default")
}

func (app *App) userID(r *http.Request) string {
	if user := server.GetUser(r); user != nil {
		return user.ID
	}
	return ""
}

var calIDRe = regexp.MustCompile(`[^a-z0-9_-]+`)

func (app *App) sanitizeCalID(s string) string {
	s = strings.TrimSpace(strings.ToLower(s))
	s = strings.Map(func(r rune) rune {
		if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '-' || r == '_' {
			return unicode.ToLower(r)
		}
		if unicode.IsSpace(r) || r == '/' || r == '.' {
			return '-'
		}
		return -1
	}, s)
	s = calIDRe.ReplaceAllString(s, "-")
	s = strings.Trim(s, "-_")
	if s == "" || s == "trash" || s == "meta" {
		return ""
	}
	if len(s) > 64 {
		s = s[:64]
	}
	return s
}

func (app *App) metaPath(uid, cal string) string {
	return filepath.Join(app.calendarDir(uid, cal), "meta.json")
}

func (app *App) loadMeta(uid, cal string) CalendarMeta {
	if app.db != nil {
		for _, m := range app.sqlCalendars(uid) {
			if m.ID == cal {
				return m
			}
		}
	}
	cal = app.sanitizeCalID(cal)
	if cal == "" {
		cal = "default"
	}
	data, err := os.ReadFile(app.metaPath(uid, cal))
	if err == nil {
		var m CalendarMeta
		if json.Unmarshal(data, &m) == nil && m.ID != "" {
			if m.Color == "" {
				m.Color = calendarTheme.Pink
			}
			return m
		}
	}
	// Defaults for directories that predate meta.json
	name := cal
	if cal == "default" {
		name = "Personal"
	}
	return CalendarMeta{ID: cal, Name: name, Color: calendarTheme.Pink, Visible: true}
}

func (app *App) saveMeta(uid string, m CalendarMeta) error {
	m.ID = app.sanitizeCalID(m.ID)
	if m.ID == "" {
		return fmt.Errorf("invalid calendar id")
	}
	if m.Name == "" {
		m.Name = m.ID
	}
	if m.Color == "" {
		m.Color = calendarTheme.Pink
	}
	if app.db != nil {
		return app.sqlSaveCalendar(uid, m)
	}
	data, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	return safefile.Write(app.metaPath(uid, m.ID), data)
}

func (app *App) ensureDefaultCalendar(uid string) {
	if app.db != nil {
		app.ensureSQLDefault(uid)
		return
	}
	dir := filepath.Join(app.calendarsDir(uid), "default")
	os.MkdirAll(dir, 0700)
	if _, err := os.Stat(filepath.Join(dir, "meta.json")); err != nil {
		_ = app.saveMeta(uid, CalendarMeta{
			ID: "default", Name: "Personal", Color: calendarTheme.Pink, Visible: true, Order: 0,
		})
	}
}

func (app *App) listCalendarMetas(uid string) []CalendarMeta {
	if app.db != nil {
		out := app.sqlCalendars(uid)
		return append(out, CalendarMeta{ID: "birthdays", Name: "Contact birthdays", Color: calendarTheme.Flamingo, Visible: true, Kind: "birthdays", ReadOnly: true})
	}
	app.ensureDefaultCalendar(uid)
	entries, _ := os.ReadDir(app.calendarsDir(uid))
	var out []CalendarMeta
	for _, e := range entries {
		if !e.IsDir() || e.Name() == "trash" {
			continue
		}
		m := app.loadMeta(uid, e.Name())
		m.ID = e.Name()
		out = append(out, m)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Order != out[j].Order {
			return out[i].Order < out[j].Order
		}
		return out[i].Name < out[j].Name
	})
	if out == nil {
		out = []CalendarMeta{}
	}
	return out
}

func (app *App) resolveCalID(uid, cal string) string {
	cal = app.sanitizeCalID(cal)
	if cal == "" {
		cal = "default"
	}
	app.ensureDefaultCalendar(uid)
	if app.db != nil {
		return cal
	}
	// Ensure directory exists
	app.calendarDir(uid, cal)
	return cal
}

// ── iCal parsing with go-ical ──

func (app *App) parseICS(path string) (Event, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Event{}, err
	}
	dec := ical.NewDecoder(bytes.NewReader(data))
	cal, err := dec.Decode()
	if err != nil {
		return Event{}, err
	}
	e := Event{ID: strings.TrimSuffix(filepath.Base(path), ".ics"), Busy: true}
	// Infer calendar id from parent dir name
	e.CalendarID = filepath.Base(filepath.Dir(path))
	events := cal.Events()
	if len(events) == 0 {
		return e, nil
	}
	ev := events[0]
	if s := ev.Props.Get(ical.PropSummary); s != nil {
		e.Title = s.Value
	}
	if s := ev.Props.Get(ical.PropDescription); s != nil {
		e.Description = s.Value
	}
	if s := ev.Props.Get(ical.PropLocation); s != nil {
		e.Location = s.Value
	}
	if s := ev.Props.Get(ical.PropColor); s != nil {
		e.Color = s.Value
	}
	if s := ev.Props.Get(ical.PropStatus); s != nil {
		e.Status = s.Value
	}
	if s := ev.Props.Get(ical.PropTransparency); s != nil {
		e.Busy = !strings.EqualFold(s.Value, "TRANSPARENT")
	}
	if s := ev.Props.Get(ical.PropRecurrenceRule); s != nil {
		e.RRule = s.Value
	}
	if s := ev.Props.Get(ical.PropCategories); s != nil {
		// CATEGORIES is a multi-value TEXT list; use library unescaping.
		if cats, err := s.TextList(); err == nil {
			for _, part := range cats {
				part = strings.TrimSpace(part)
				if part != "" {
					e.Categories = append(e.Categories, part)
				}
			}
		}
	}
	for _, p := range ev.Props.Values(ical.PropExceptionDates) {
		// EXDATE may be comma-separated multi-value TEXT/DATE-TIME.
		parts := strings.Split(p.Value, ",")
		for _, part := range parts {
			part = strings.TrimSpace(part)
			if part == "" {
				continue
			}
			if ts := app.parseICSDate(part); ts != 0 {
				e.ExDates = append(e.ExDates, ts)
			}
		}
	}
	if s := ev.Props.Get(ical.PropDateTimeStart); s != nil {
		e.Start = app.parseICSDate(s.Value)
		if len(s.Value) == 8 {
			e.AllDay = true
		}
		if tz := s.Params.Get("TZID"); tz != "" {
			e.Timezone = tz
		}
	}
	if s := ev.Props.Get(ical.PropDateTimeEnd); s != nil {
		e.End = app.parseICSDate(s.Value)
	}
	// VALARM children → minutes-before reminders
	for _, child := range ev.Children {
		if child.Name != ical.CompAlarm {
			continue
		}
		if tr := child.Props.Get(ical.PropTrigger); tr != nil {
			if m, ok := app.parseAlarmTriggerMinutes(tr.Value); ok {
				e.Reminders = append(e.Reminders, m)
			}
		}
	}
	return e, nil
}

// parseICSBytes is used by CalDAV PUT so database-backed calendars never need
// a temporary filesystem representation.
func (app *App) parseICSBytes(data []byte, id, calendarID string) (Event, error) {
	cal, err := ical.NewDecoder(bytes.NewReader(data)).Decode()
	if err != nil {
		return Event{}, err
	}
	events := cal.Events()
	if len(events) == 0 {
		return Event{}, fmt.Errorf("calendar data contains no event")
	}
	e, ok := app.subscriptionEvent(events[0], calendarID, 0)
	if !ok {
		return Event{}, fmt.Errorf("event has no DTSTART")
	}
	if id != "" {
		e.ID = id
	}
	return e, nil
}

// parseAlarmTriggerMinutes parses TRIGGER values like -PT15M, -PT1H, -P1D.
// Returns minutes before start (positive).
func (app *App) parseAlarmTriggerMinutes(v string) (int, bool) {
	v = strings.TrimSpace(v)
	if v == "" {
		return 0, false
	}
	neg := strings.HasPrefix(v, "-")
	if neg {
		v = v[1:]
	}
	// Only relative triggers; absolute DATE-TIME ignored for now.
	if !strings.HasPrefix(v, "P") {
		return 0, false
	}
	v = v[1:] // strip P
	minutes := 0
	// Forms: T15M, T1H30M, 1D, 1DT2H
	dayPart, timePart := v, ""
	if i := strings.Index(v, "T"); i >= 0 {
		dayPart, timePart = v[:i], v[i+1:]
	}
	if dayPart != "" {
		if strings.HasSuffix(dayPart, "D") {
			n, _ := strconv.Atoi(strings.TrimSuffix(dayPart, "D"))
			minutes += n * 24 * 60
		} else if strings.HasSuffix(dayPart, "W") {
			n, _ := strconv.Atoi(strings.TrimSuffix(dayPart, "W"))
			minutes += n * 7 * 24 * 60
		}
	}
	// Parse timePart greedily: 1H30M15S
	rest := timePart
	for rest != "" {
		i := 0
		for i < len(rest) && rest[i] >= '0' && rest[i] <= '9' {
			i++
		}
		if i == 0 || i >= len(rest) {
			break
		}
		n, _ := strconv.Atoi(rest[:i])
		unit := rest[i]
		rest = rest[i+1:]
		switch unit {
		case 'H':
			minutes += n * 60
		case 'M':
			minutes += n
		case 'S':
			// ignore sub-minute
		default:
			return 0, false
		}
	}
	if minutes == 0 && timePart == "" && dayPart == "" {
		return 0, false
	}
	if !neg {
		// Positive trigger = after start; store as 0 (at start) for simplicity
		return 0, true
	}
	return minutes, true
}

func (app *App) formatAlarmTrigger(minutesBefore int) string {
	if minutesBefore <= 0 {
		return "-PT0M"
	}
	if minutesBefore%1440 == 0 {
		return fmt.Sprintf("-P%dD", minutesBefore/1440)
	}
	if minutesBefore%60 == 0 {
		return fmt.Sprintf("-PT%dH", minutesBefore/60)
	}
	return fmt.Sprintf("-PT%dM", minutesBefore)
}

func (app *App) parseICSDate(s string) int64 {
	// All-day DATE values are civil dates — interpret as local midnight so the
	// day matches what the user picked in the browser (which sends local midnight).
	if len(s) == 8 {
		t, err := time.ParseInLocation("20060102", s, time.Local)
		if err != nil {
			return 0
		}
		return t.Unix()
	}
	format := "20060102T150405Z"
	if len(s) == 15 && !strings.HasSuffix(s, "Z") {
		format = "20060102T150405"
	}
	t, err := time.Parse(format, s)
	if err != nil {
		return 0
	}
	return t.Unix()
}

func (app *App) toICS(e Event) string {
	cal := ical.NewCalendar()
	cal.Props.SetText(ical.PropVersion, "2.0")
	cal.Props.SetText(ical.PropProductID, "-//Dar//Calendar//EN")

	ev := ical.NewEvent()
	ev.Props.SetText(ical.PropUID, e.ID)
	ev.Props.SetText(ical.PropDateTimeStamp, time.Now().UTC().Format("20060102T150405Z"))
	cal.Children = append(cal.Children, ev.Component)

	if e.AllDay {
		// Write civil DATE in local timezone so re-read lands on the same day.
		ev.Props.SetText(ical.PropDateTimeStart, time.Unix(e.Start, 0).In(time.Local).Format("20060102"))
		ev.Props.SetText(ical.PropDateTimeEnd, time.Unix(e.End, 0).In(time.Local).Format("20060102"))
	} else {
		ev.Props.SetText(ical.PropDateTimeStart, time.Unix(e.Start, 0).UTC().Format("20060102T150405Z"))
		ev.Props.SetText(ical.PropDateTimeEnd, time.Unix(e.End, 0).UTC().Format("20060102T150405Z"))
	}
	if e.Title != "" {
		ev.Props.SetText(ical.PropSummary, e.Title)
	}
	if e.Description != "" {
		ev.Props.SetText(ical.PropDescription, e.Description)
	}
	if e.Location != "" {
		ev.Props.SetText(ical.PropLocation, e.Location)
	}
	if e.Color != "" {
		ev.Props.SetText(ical.PropColor, e.Color)
	}
	if e.Status != "" {
		ev.Props.SetText(ical.PropStatus, e.Status)
	} else {
		ev.Props.SetText(ical.PropStatus, "CONFIRMED")
	}
	if e.Busy {
		ev.Props.SetText(ical.PropTransparency, "OPAQUE")
	} else {
		ev.Props.SetText(ical.PropTransparency, "TRANSPARENT")
	}
	if e.RRule != "" {
		ev.Props.SetText(ical.PropRecurrenceRule, e.RRule)
	}
	for _, ex := range e.ExDates {
		p := ical.NewProp(ical.PropExceptionDates)
		if e.AllDay {
			p.SetText(time.Unix(ex, 0).In(time.Local).Format("20060102"))
		} else {
			p.SetText(time.Unix(ex, 0).UTC().Format("20060102T150405Z"))
		}
		ev.Props.Add(p)
	}
	if len(e.Categories) > 0 {
		// Multi-value TEXT: commas separate categories; library escapes internal commas.
		p := ical.NewProp(ical.PropCategories)
		p.SetTextList(e.Categories)
		ev.Props.Set(p)
	}
	for _, mins := range e.Reminders {
		if mins < 0 {
			continue
		}
		alarm := ical.NewComponent(ical.CompAlarm)
		alarm.Props.SetText(ical.PropAction, "DISPLAY")
		alarm.Props.SetText(ical.PropDescription, e.Title)
		alarm.Props.SetText(ical.PropTrigger, app.formatAlarmTrigger(mins))
		ev.Children = append(ev.Children, alarm)
	}

	var b strings.Builder
	enc := ical.NewEncoder(&b)
	_ = enc.Encode(cal)
	return b.String()
}

func (app *App) applyCalendarColor(uid string, e *Event) {
	if e.Color != "" {
		return
	}
	m := app.loadMeta(uid, e.CalendarID)
	e.Color = m.Color
}

func (app *App) listEventsInCalendar(uid, cal string) []Event {
	if cal == "birthdays" {
		return app.birthdayEvents(uid)
	}
	if app.db != nil {
		return app.sqlEvents(uid, cal)
	}
	dir := app.calendarDir(uid, cal)
	entries, _ := os.ReadDir(dir)
	var events []Event
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".ics") {
			continue
		}
		e, err := app.parseICS(filepath.Join(dir, entry.Name()))
		if err != nil {
			continue
		}
		e.CalendarID = cal
		app.applyCalendarColor(uid, &e)
		events = append(events, e)
	}
	return events
}

func (app *App) listAllEvents(uid string) []Event {
	if app.db != nil {
		return append(app.sqlEvents(uid, ""), app.birthdayEvents(uid)...)
	}
	var events []Event
	for _, m := range app.listCalendarMetas(uid) {
		events = append(events, app.listEventsInCalendar(uid, m.ID)...)
	}
	return events
}

func (app *App) listAllEventsInRange(uid string, start, end int64) []Event {
	if app.db != nil {
		return append(app.sqlEventsInRange(uid, "", start, end), app.birthdayEvents(uid)...)
	}
	return app.listAllEvents(uid)
}

func (app *App) findEvent(uid, id string) (Event, string, error) {
	if !validEventID(id) {
		return Event{}, "", os.ErrNotExist
	}
	if app.db != nil {
		return app.sqlFindEvent(uid, id)
	}
	// Prefer explicit calendar search order; scan all calendars
	for _, m := range app.listCalendarMetas(uid) {
		path := filepath.Join(app.calendarDir(uid, m.ID), id+".ics")
		e, err := app.parseICS(path)
		if err == nil {
			e.CalendarID = m.ID
			app.applyCalendarColor(uid, &e)
			return e, m.ID, nil
		}
	}
	return Event{}, "", os.ErrNotExist
}

func (app *App) filterByRange(events []Event, start, end int64) []Event {
	if start == 0 && end == 0 {
		return events
	}
	var filtered []Event
	for _, e := range events {
		if e.Start < end && e.End > start {
			filtered = append(filtered, e)
		}
	}
	return filtered
}

// ── REST: calendars ──

func (app *App) handleCalendars(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	switch r.Method {
	case http.MethodGet:
		app.writeJSON(w, map[string]any{"calendars": app.listCalendarMetas(uid)})
	case http.MethodPost:
		var body struct {
			Name  string `json:"name"`
			Color string `json:"color"`
			ID    string `json:"id"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			http.Error(w, "invalid json", 400)
			return
		}
		id := body.ID
		if id == "" {
			id = app.sanitizeCalID(body.Name)
		} else {
			id = app.sanitizeCalID(id)
		}
		if id == "" {
			http.Error(w, "invalid calendar name", 400)
			return
		}
		// Avoid clobbering existing
		if app.db != nil {
			for _, existing := range app.sqlCalendars(uid) {
				if existing.ID == id {
					http.Error(w, "calendar already exists", http.StatusConflict)
					return
				}
			}
		}
		if _, err := os.Stat(app.metaPath(uid, id)); err == nil {
			http.Error(w, "calendar already exists", 409)
			return
		}
		// Also check dir with events but no meta
		if entries, _ := os.ReadDir(filepath.Join(app.calendarsDir(uid), id)); len(entries) > 0 {
			// allow if only empty? treat as exists if dir non-empty
			if _, err := os.Stat(filepath.Join(app.calendarsDir(uid), id)); err == nil {
				// if meta missing, just write meta
			}
		}
		metas := app.listCalendarMetas(uid)
		order := len(metas)
		m := CalendarMeta{
			ID: id, Name: body.Name, Color: body.Color, Visible: true, Order: order,
		}
		if m.Name == "" {
			m.Name = id
		}
		if m.Color == "" {
			m.Color = calendarTheme.Pink
		}
		if err := app.saveMeta(uid, m); err != nil {
			log.Printf("create calendar metadata: %v", err)
			http.Error(w, "calendar creation failed", http.StatusInternalServerError)
			return
		}
		app.recordActivity(r.Context(), uid, "calendar_created", "created calendar "+m.Name, "calendar", m.ID, m.Name, "api")
		app.writeJSON(w, m)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleCalendarUpdate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	var body CalendarMeta
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "invalid json", 400)
		return
	}
	body.ID = app.sanitizeCalID(body.ID)
	if body.ID == "" {
		http.Error(w, "missing id", 400)
		return
	}
	if body.ID == "birthdays" {
		http.Error(w, "generated birthday calendar is read-only", http.StatusForbidden)
		return
	}
	if app.db != nil {
		var current CalendarMeta
		for _, meta := range app.sqlCalendars(uid) {
			if meta.ID == body.ID {
				current = meta
				break
			}
		}
		if current.ID == "" {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		if body.Name != "" {
			current.Name = body.Name
		}
		if body.Color != "" {
			current.Color = body.Color
		}
		current.Visible = body.Visible
		current.Order = body.Order
		if err := app.sqlUpdateCalendar(uid, current); err != nil {
			http.Error(w, "update failed", http.StatusInternalServerError)
			return
		}
		app.recordActivity(r.Context(), uid, "calendar_updated", "updated calendar "+current.Name, "calendar", current.ID, current.Name, "api")
		app.writeJSON(w, current)
		return
	}
	// Ensure calendar exists
	if _, err := os.Stat(app.calendarDir(uid, body.ID)); err != nil {
		http.Error(w, "not found", 404)
		return
	}
	cur := app.loadMeta(uid, body.ID)
	if body.Name != "" {
		cur.Name = body.Name
	}
	if body.Color != "" {
		cur.Color = body.Color
	}
	// Visible is bool — accept from body always (client sends full object)
	cur.Visible = body.Visible
	cur.Order = body.Order
	cur.ID = body.ID
	if err := app.saveMeta(uid, cur); err != nil {
		log.Printf("update calendar metadata: %v", err)
		http.Error(w, "calendar update failed", http.StatusInternalServerError)
		return
	}
	app.recordActivity(r.Context(), uid, "calendar_updated", "updated calendar "+cur.Name, "calendar", cur.ID, cur.Name, "api")
	app.writeJSON(w, cur)
}

func (app *App) handleCalendarDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	id := app.sanitizeCalID(r.URL.Query().Get("id"))
	if id == "" || id == "default" || id == "birthdays" {
		http.Error(w, "cannot delete default calendar", 400)
		return
	}
	dir := filepath.Join(app.calendarsDir(uid), id)
	if app.db != nil {
		if err := app.withConn(func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `DELETE FROM calendars WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}})
		}); err != nil {
			http.Error(w, "delete failed", 500)
			return
		}
		app.recordActivity(r.Context(), uid, "calendar_deleted", "deleted calendar "+id, "calendar", id, id, "api")
		app.writeJSON(w, map[string]string{"status": "ok"})
		return
	}
	if err := os.RemoveAll(dir); err != nil {
		http.Error(w, "not found", 404)
		return
	}
	app.recordActivity(r.Context(), uid, "calendar_deleted", "deleted calendar "+id, "calendar", id, id, "api")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

// ── REST: events ──

func (app *App) handleList(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	calFilter := r.URL.Query().Get("calendar")
	var events []Event
	startStr := r.URL.Query().Get("start")
	endStr := r.URL.Query().Get("end")
	var start, end int64
	if startStr != "" && endStr != "" {
		fmt.Sscanf(startStr, "%d", &start)
		fmt.Sscanf(endStr, "%d", &end)
	}
	if calFilter != "" {
		calID := app.resolveCalID(uid, calFilter)
		if app.db != nil && start != 0 && end != 0 {
			events = app.sqlEventsInRange(uid, calID, start, end)
		} else {
			events = app.listEventsInCalendar(uid, calID)
		}
	} else if app.db != nil && start != 0 && end != 0 {
		events = app.listAllEventsInRange(uid, start, end)
	} else {
		events = app.listAllEvents(uid)
	}
	if startStr != "" && endStr != "" {
		// Expand RRULE masters into instances for the visible range.
		events = app.expandInRange(events, start, end)
	} else {
		events = app.filterByRange(events, 0, 0)
	}
	if events == nil {
		events = []Event{}
	}
	app.writeJSON(w, map[string]any{"events": events})
}

func (app *App) handleGet(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	id := r.URL.Query().Get("id")
	cal := r.URL.Query().Get("calendar")
	if cal != "" {
		e, err := app.parseICS(filepath.Join(app.calendarDir(uid, cal), id+".ics"))
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		e.CalendarID = app.resolveCalID(uid, cal)
		app.applyCalendarColor(uid, &e)
		app.writeJSON(w, e)
		return
	}
	e, _, err := app.findEvent(uid, id)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	app.writeJSON(w, e)
}

func (app *App) handleCreate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "bad body", 400)
		return
	}
	var e Event
	if err := json.Unmarshal(body, &e); err != nil {
		http.Error(w, "invalid json", 400)
		return
	}
	e.CalendarID = app.resolveCalID(uid, e.CalendarID)
	if e.CalendarID == "birthdays" {
		http.Error(w, "generated birthday calendar is read-only", http.StatusForbidden)
		return
	}
	if e.ID == "" {
		e.ID, err = newCalendarID()
		if err != nil {
			http.Error(w, "could not create event", http.StatusInternalServerError)
			return
		}
	}
	if e.Color == "" {
		e.Color = app.loadMeta(uid, e.CalendarID).Color
	}
	if e.Status == "" {
		e.Status = "CONFIRMED"
	}
	// Default busy=true when client omits the field (JSON bool zero is false).
	if !bytes.Contains(body, []byte(`"busy"`)) {
		e.Busy = true
	}
	if e.End == 0 {
		e.End = e.Start + 3600
	}
	if err := app.writeEvent(uid, e); err != nil {
		http.Error(w, "write failed", 500)
		return
	}
	app.recordActivity(r.Context(), uid, "event_created", "created event "+e.Title, "event", e.ID, e.Title, "api")
	app.writeJSON(w, e)
}

func (app *App) handleUpdate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "bad body", 400)
		return
	}
	var e Event
	if err := json.Unmarshal(body, &e); err != nil {
		http.Error(w, "invalid json", 400)
		return
	}
	if e.ID == "" {
		http.Error(w, "missing id", 400)
		return
	}

	scope := strings.ToLower(strings.TrimSpace(e.Scope))
	if scope == "" {
		scope = "all"
	}
	masterID := e.MasterID
	if masterID == "" {
		masterID = app.masterIDFrom(e.ID)
	}
	recID := e.RecurrenceID
	if recID == 0 {
		if i := strings.LastIndex(e.ID, "@"); i > 0 {
			if n, err := strconv.ParseInt(e.ID[i+1:], 10, 64); err == nil {
				recID = n
			}
		}
	}

	existing, oldCal, err := app.findEvent(uid, masterID)
	busyInBody := bytes.Contains(body, []byte(`"busy"`))
	if err != nil {
		// Fall back to literal id (non-expanded)
		existing, oldCal, err = app.findEvent(uid, e.ID)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		masterID = e.ID
	}

	if e.CalendarID == "" {
		e.CalendarID = oldCal
	}
	if e.CalendarID == "birthdays" || oldCal == "birthdays" {
		http.Error(w, "generated birthday calendar is read-only", http.StatusForbidden)
		return
	}
	if e.Color == "" {
		e.Color = existing.Color
	}
	if !busyInBody {
		e.Busy = existing.Busy
	}
	if e.Location == "" && existing.Location != "" && !bytes.Contains(body, []byte(`"location"`)) {
		e.Location = existing.Location
	}
	e.CalendarID = app.resolveCalID(uid, e.CalendarID)
	if e.Color == "" {
		e.Color = app.loadMeta(uid, e.CalendarID).Color
	}
	if e.Status == "" {
		e.Status = "CONFIRMED"
	}

	// Clear request-only fields before persist.
	e.Scope = ""
	e.MasterID = ""
	// Keep RecurrenceID only on expanded instances (not written to master).

	isRecurring := existing.RRule != ""
	if isRecurring && recID != 0 && (scope == "this" || scope == "future") {
		switch scope {
		case "this":
			// Exclude this occurrence from the series; create a standalone event.
			master := existing
			master.ExDates = app.appendExDate(master.ExDates, recID, master.AllDay)
			if err := app.writeEvent(uid, master); err != nil {
				http.Error(w, "write failed", 500)
				return
			}
			e.ID, err = newCalendarID()
			if err != nil {
				http.Error(w, "could not create event", http.StatusInternalServerError)
				return
			}
			e.RRule = ""
			e.ExDates = nil
			e.RecurrenceID = 0
			if err := app.writeEvent(uid, e); err != nil {
				http.Error(w, "write failed", 500)
				return
			}
			app.recordActivity(r.Context(), uid, "event_updated", "updated event "+e.Title, "event", e.ID, e.Title, "api")
			app.writeJSON(w, e)
			return
		case "future":
			// Split series: master ends before this instance; new series from here.
			if recID > existing.Start {
				master := existing
				master.RRule = app.rruleWithUntil(master.RRule, recID, master.AllDay)
				if err := app.writeEvent(uid, master); err != nil {
					http.Error(w, "write failed", 500)
					return
				}
			} else {
				// First instance + future == update entire series.
				scope = "all"
			}
			if scope == "future" {
				e.ID, err = newCalendarID()
				if err != nil {
					http.Error(w, "could not create event", http.StatusInternalServerError)
					return
				}
				e.ExDates = nil
				e.RecurrenceID = 0
				if e.RRule == "" {
					e.RRule = existing.RRule
					// Strip UNTIL/COUNT from inherited rule for open-ended future series.
					if ropt, err := app.parseRRuleStripUntil(e.RRule); err == nil && ropt != "" {
						e.RRule = ropt
					}
				}
				if err := app.writeEvent(uid, e); err != nil {
					http.Error(w, "write failed", 500)
					return
				}
				app.recordActivity(r.Context(), uid, "event_updated", "updated event "+e.Title, "event", e.ID, e.Title, "api")
				app.writeJSON(w, e)
				return
			}
		}
	}

	// scope=all (default): update the master event.
	e.ID = masterID
	e.RecurrenceID = 0
	if !bytes.Contains(body, []byte(`"exdates"`)) {
		e.ExDates = existing.ExDates
	}
	// For "edit all" on a recurring series, keep original DTSTART if client sent an instance time
	// but only changed metadata — if rrule still set and start equals instance, restore master start.
	// Simpler: trust client payload for all fields.
	if e.RRule == "" && existing.RRule != "" && !bytes.Contains(body, []byte(`"rrule"`)) {
		e.RRule = existing.RRule
	}
	if !bytes.Contains(body, []byte(`"reminders"`)) && len(e.Reminders) == 0 && len(existing.Reminders) > 0 {
		e.Reminders = existing.Reminders
	}

	if oldCal != "" && oldCal != e.CalendarID {
		app.removeEventFile(uid, oldCal, masterID)
	}
	if err := app.writeEvent(uid, e); err != nil {
		http.Error(w, "write failed", 500)
		return
	}
	app.recordActivity(r.Context(), uid, "event_updated", "updated event "+e.Title, "event", e.ID, e.Title, "api")
	app.writeJSON(w, e)
}

func (app *App) parseRRuleStripUntil(s string) (string, error) {
	// Drop UNTIL and COUNT so a split future series continues indefinitely.
	parts := strings.Split(s, ";")
	var keep []string
	for _, p := range parts {
		u := strings.ToUpper(p)
		if strings.HasPrefix(u, "UNTIL=") || strings.HasPrefix(u, "COUNT=") {
			continue
		}
		if p != "" {
			keep = append(keep, p)
		}
	}
	return strings.Join(keep, ";"), nil
}

func (app *App) handleDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	id := r.URL.Query().Get("id")
	cal := r.URL.Query().Get("calendar")
	scope := strings.ToLower(r.URL.Query().Get("scope"))
	if scope == "" {
		scope = "all"
	}
	recStr := r.URL.Query().Get("recurrence_id")
	var recID int64
	if recStr != "" {
		fmt.Sscanf(recStr, "%d", &recID)
	}
	masterID := app.masterIDFrom(id)
	if recID == 0 {
		if i := strings.LastIndex(id, "@"); i > 0 {
			fmt.Sscanf(id[i+1:], "%d", &recID)
		}
	}

	if cal != "" {
		// Explicit calendar path — only full delete of that file id
		_ = os.Remove(filepath.Join(app.calendarDir(uid, cal), app.masterIDFrom(id)+".ics"))
		app.recordActivity(r.Context(), uid, "event_deleted", "deleted event "+id, "event", id, id, "api")
		app.writeJSON(w, map[string]string{"status": "ok"})
		return
	}

	existing, calID, err := app.findEvent(uid, masterID)
	if err != nil {
		// try literal
		if _, calID2, err2 := app.findEvent(uid, id); err2 == nil {
			app.removeEventFile(uid, calID2, id)
		}
		app.recordActivity(r.Context(), uid, "event_deleted", "deleted event "+id, "event", id, id, "api")
		app.writeJSON(w, map[string]string{"status": "ok"})
		return
	}

	if existing.RRule != "" && recID != 0 && scope == "this" {
		existing.ExDates = app.appendExDate(existing.ExDates, recID, existing.AllDay)
		_ = app.writeEvent(uid, existing)
		app.recordActivity(r.Context(), uid, "event_deleted", "deleted occurrence of "+existing.Title, "event", existing.ID, existing.Title, "api")
		app.writeJSON(w, map[string]string{"status": "ok"})
		return
	}
	if existing.RRule != "" && recID != 0 && scope == "future" {
		if recID > existing.Start {
			existing.RRule = app.rruleWithUntil(existing.RRule, recID, existing.AllDay)
			_ = app.writeEvent(uid, existing)
			app.recordActivity(r.Context(), uid, "event_deleted", "deleted future occurrences of "+existing.Title, "event", existing.ID, existing.Title, "api")
			app.writeJSON(w, map[string]string{"status": "ok"})
			return
		}
	}
	// all
	app.removeEventFile(uid, calID, masterID)
	app.recordActivity(r.Context(), uid, "event_deleted", "deleted event "+existing.Title, "event", existing.ID, existing.Title, "api")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	q := strings.ToLower(r.URL.Query().Get("q"))
	var results []Event
	for _, ev := range app.listAllEvents(uid) {
		hay := strings.ToLower(ev.Title + " " + ev.Description + " " + ev.Location)
		if q == "" || strings.Contains(hay, q) {
			results = append(results, ev)
		}
	}
	if results == nil {
		results = []Event{}
	}
	app.writeJSON(w, map[string]any{"events": results})
}

func (app *App) handleHealth(w http.ResponseWriter, r *http.Request) {
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleImport(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	cal := app.resolveCalID(uid, r.URL.Query().Get("calendar"))
	r.Body = http.MaxBytesReader(w, r.Body, 10<<20)
	if err := r.ParseMultipartForm(10 << 20); err != nil {
		http.Error(w, "invalid request", 400)
		return
	}
	file, _, err := r.FormFile("file")
	if err != nil {
		http.Error(w, "missing file", 400)
		return
	}
	defer file.Close()
	if app.db != nil {
		data, err := io.ReadAll(file)
		if err != nil {
			http.Error(w, "read failed", 400)
			return
		}
		dec := ical.NewDecoder(bytes.NewReader(data))
		imported := 0
		for {
			parsed, err := dec.Decode()
			if err == io.EOF {
				break
			}
			if err != nil {
				http.Error(w, "invalid calendar data", 400)
				return
			}
			for _, component := range parsed.Events() {
				event, ok := app.subscriptionEvent(component, cal, imported)
				if !ok {
					continue
				}
				event.ID, err = newCalendarID()
				if err != nil {
					http.Error(w, "could not import event", http.StatusInternalServerError)
					return
				}
				event.CalendarID = cal
				if err := app.writeEvent(uid, event); err != nil {
					http.Error(w, "write failed", 500)
					return
				}
				imported++
			}
		}
		if imported == 0 {
			http.Error(w, "no events found", 400)
			return
		}
		app.recordActivity(r.Context(), uid, "events_imported", fmt.Sprintf("imported %d events", imported), "calendar", cal, cal, "api")
		app.writeJSON(w, map[string]any{"imported": imported, "calendar": cal})
		return
	}

	dir := app.calendarDir(uid, cal)
	data, _ := io.ReadAll(file)
	dec := ical.NewDecoder(bytes.NewReader(data))
	imported := 0
	for {
		icalCal, err := dec.Decode()
		if err == io.EOF {
			break
		}
		if err != nil {
			continue
		}
		for _, component := range icalCal.Children {
			if component.Name != ical.CompEvent {
				continue
			}
			id, err := newCalendarID()
			if err != nil {
				http.Error(w, "could not import event", http.StatusInternalServerError)
				return
			}
			var b strings.Builder
			outCal := ical.NewCalendar()
			outCal.Props.SetText(ical.PropVersion, "2.0")
			outCal.Props.SetText(ical.PropProductID, "-//Dar//Calendar//EN")
			outEv := ical.NewEvent()
			uidStr := id
			if u := component.Props.Get(ical.PropUID); u != nil {
				uidStr = u.Value
			}
			outEv.Props.SetText(ical.PropUID, uidStr)
			outEv.Props.SetText(ical.PropDateTimeStamp, time.Now().UTC().Format("20060102T150405Z"))
			for _, plist := range component.Props {
				for _, p := range plist {
					if p.Name == ical.PropUID || p.Name == ical.PropDateTimeStamp {
						continue
					}
					outEv.Props.Set(&p)
				}
			}
			outCal.Children = append(outCal.Children, outEv.Component)
			enc := ical.NewEncoder(&b)
			_ = enc.Encode(outCal)
			if err := safefile.Write(filepath.Join(dir, uidStr+".ics"), []byte(b.String())); err != nil {
				http.Error(w, "write failed", 500)
				return
			}
			imported++
		}
	}
	if imported == 0 {
		http.Error(w, "no events found", 400)
		return
	}
	app.recordActivity(r.Context(), uid, "events_imported", fmt.Sprintf("imported %d events", imported), "calendar", cal, cal, "api")
	app.writeJSON(w, map[string]any{"imported": imported, "calendar": cal})
}

func (app *App) handleExport(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	cal := r.URL.Query().Get("calendar")
	var dirs []string
	if cal != "" {
		dirs = []string{app.calendarDir(uid, cal)}
	} else {
		for _, m := range app.listCalendarMetas(uid) {
			dirs = append(dirs, app.calendarDir(uid, m.ID))
		}
	}

	w.Header().Set("Content-Type", ical.MIMEType)
	w.Header().Set("Content-Disposition", "attachment; filename=\"calendar.ics\"")
	if app.db != nil {
		var events []Event
		if cal != "" {
			events = app.listEventsInCalendar(uid, app.resolveCalID(uid, cal))
		} else {
			events = app.listAllEvents(uid)
		}
		for _, event := range events {
			_, _ = w.Write([]byte(app.toICS(event)))
		}
		return
	}
	for _, dir := range dirs {
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".ics") {
				continue
			}
			data, err := os.ReadFile(filepath.Join(dir, e.Name()))
			if err != nil {
				continue
			}
			_, _ = w.Write(data)
		}
	}
}

// ── CalDAV — WebDAV/CalDAV protocol ──

func (app *App) handleCalDAV(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	path := strings.TrimPrefix(r.URL.Path, "/caldav")
	path = strings.TrimPrefix(path, "/")
	if path == "" {
		path = uid
	}

	// Path format: /caldav/{user}/{calendar}/{uid}.ics.
	parts := strings.SplitN(path, "/", 4)
	if parts[0] != uid {
		http.NotFound(w, r)
		return
	}
	switch r.Method {
	case "MKCALENDAR":
		if len(parts) < 2 || parts[1] == "" {
			http.Error(w, "bad path", 400)
			return
		}
		if app.db == nil {
			app.calendarDir(parts[0], parts[1])
		}
		_ = app.saveMeta(parts[0], CalendarMeta{
			ID: app.sanitizeCalID(parts[1]), Name: parts[1], Color: calendarTheme.Pink, Visible: true,
		})
		w.WriteHeader(http.StatusCreated)

	case "PROPFIND":
		app.handlePROPFIND(w, r, parts)

	case "REPORT":
		app.handleREPORT(w, r, parts)

	case "PUT":
		app.handleCalDAVPut(w, r, parts)

	case "GET":
		app.handleCalDAVGet(w, r, parts)

	case "DELETE":
		app.handleCalDAVDelete(w, r, parts)

	case "OPTIONS":
		w.Header().Set("Allow", "OPTIONS, GET, PUT, DELETE, PROPFIND, REPORT, MKCALENDAR")
		w.Header().Set("DAV", "1, 2, calendar-access, addressbook")

	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handlePROPFIND(w http.ResponseWriter, r *http.Request, parts []string) {
	uid := parts[0]
	if uid == "" {
		uid = "default"
	}
	calName := ""
	if len(parts) > 1 {
		calName = parts[1]
	}

	type propfind struct {
		Resourcetype struct {
			Collection string `xml:"d:collection,omitempty"`
			Calendar   string `xml:"cal:calendar,omitempty"`
		} `xml:"d:resourcetype"`
		Displayname string `xml:"d:displayname,omitempty"`
		GetETag     string `xml:"d:getetag,omitempty"`
		GetCTag     string `xml:"cs:getctag,omitempty"`
	}
	type propfindResp struct {
		HREF     string `xml:"d:href"`
		PropStat struct {
			Status string   `xml:"d:status"`
			Prop   propfind `xml:"d:prop"`
		} `xml:"d:propstat"`
	}
	type multistatus struct {
		XMLName  xml.Name       `xml:"D:multistatus"`
		XMLNSD   string         `xml:"xmlns:D,attr"`
		XMLNSCal string         `xml:"xmlns:cal,attr"`
		XMLNSCS  string         `xml:"xmlns:cs,attr"`
		Response []propfindResp `xml:"d:response"`
	}

	ms := multistatus{
		XMLNSD:   "DAV:",
		XMLNSCal: "urn:ietf:params:xml:ns:caldav",
		XMLNSCS:  "http://calendarserver.org/ns/",
	}

	if calName == "" {
		app.ensureDefaultCalendar(uid)
		for _, m := range app.listCalendarMetas(uid) {
			p := propfind{Displayname: m.Name}
			p.Resourcetype.Collection = "collection"
			ms.Response = append(ms.Response, propfindResp{
				HREF: "/" + uid + "/" + m.ID + "/",
				PropStat: struct {
					Status string   `xml:"d:status"`
					Prop   propfind `xml:"d:prop"`
				}{
					Status: "HTTP/1.1 200 OK",
					Prop:   p,
				},
			})
		}
	} else {
		if app.db != nil {
			for _, event := range app.listEventsInCalendar(uid, calName) {
				p := propfind{GetETag: fmt.Sprintf(`"%d"`, event.Start+event.End)}
				ms.Response = append(ms.Response, propfindResp{HREF: "/" + uid + "/" + calName + "/" + event.ID + ".ics", PropStat: struct {
					Status string   `xml:"d:status"`
					Prop   propfind `xml:"d:prop"`
				}{Status: "HTTP/1.1 200 OK", Prop: p}})
			}
			w.Header().Set("Content-Type", "application/xml; charset=utf-8")
			w.WriteHeader(http.StatusMultiStatus)
			_ = xml.NewEncoder(w).Encode(ms)
			return
		}
		dir := app.calendarDir(uid, calName)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".ics") {
				continue
			}
			fi, _ := e.Info()
			etag := ""
			if fi != nil {
				etag = fmt.Sprintf(`"%d-%d"`, fi.ModTime().Unix(), fi.Size())
			}
			p := propfind{GetETag: etag}
			ms.Response = append(ms.Response, propfindResp{
				HREF: "/" + uid + "/" + calName + "/" + e.Name(),
				PropStat: struct {
					Status string   `xml:"d:status"`
					Prop   propfind `xml:"d:prop"`
				}{
					Status: "HTTP/1.1 200 OK",
					Prop:   p,
				},
			})
		}
	}

	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	xml.NewEncoder(w).Encode(ms)
}

func (app *App) handleREPORT(w http.ResponseWriter, r *http.Request, parts []string) {
	uid := parts[0]
	calName := ""
	if len(parts) > 1 {
		calName = parts[1]
	}
	dir := app.calendarDir(uid, calName)

	body, _ := io.ReadAll(r.Body)
	bodyStr := string(body)

	var startTS, endTS int64
	if si := strings.Index(bodyStr, "start=\""); si >= 0 {
		start := bodyStr[si+7:]
		if ei := strings.Index(start, "\""); ei >= 0 {
			start = start[:ei]
			if t, err := time.Parse("20060102T150405Z", start); err == nil {
				startTS = t.Unix()
			}
		}
	}
	if si := strings.Index(bodyStr, "end=\""); si >= 0 {
		end := bodyStr[si+5:]
		if ei := strings.Index(end, "\""); ei >= 0 {
			end = end[:ei]
			if t, err := time.Parse("20060102T150405Z", end); err == nil {
				endTS = t.Unix()
			}
		}
	}

	var results []Event
	if app.db != nil {
		results = app.listEventsInCalendar(uid, calName)
	} else {
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".ics") {
				continue
			}
			ev, err := app.parseICS(filepath.Join(dir, e.Name()))
			if err == nil {
				results = append(results, ev)
			}
		}
	}
	if startTS != 0 || endTS != 0 {
		results = app.filterByRange(results, startTS, endTS)
	}
	if results == nil {
		results = []Event{}
	}

	type reportResp struct {
		HREF     string `xml:"d:href"`
		PropStat struct {
			Status string `xml:"d:status"`
			Prop   struct {
				CalendarData string `xml:"cal:calendar-data"`
				GetETag      string `xml:"d:getetag"`
			} `xml:"d:prop"`
		} `xml:"d:propstat"`
	}
	type multistatus struct {
		XMLNSD   string       `xml:"xmlns:D,attr"`
		XMLNSCal string       `xml:"xmlns:cal,attr"`
		Response []reportResp `xml:"d:response"`
	}
	ms := multistatus{
		XMLNSD:   "DAV:",
		XMLNSCal: "urn:ietf:params:xml:ns:caldav",
	}
	for _, ev := range results {
		data := []byte(app.toICS(ev))
		etag := fmt.Sprintf(`"%d"`, ev.Start+ev.End)
		if app.db == nil {
			data, _ = os.ReadFile(filepath.Join(dir, ev.ID+".ics"))
			if fi, _ := os.Stat(filepath.Join(dir, ev.ID+".ics")); fi != nil {
				etag = fmt.Sprintf(`"%d-%d"`, fi.ModTime().Unix(), fi.Size())
			}
		}
		ms.Response = append(ms.Response, reportResp{
			HREF: "/" + uid + "/" + calName + "/" + ev.ID + ".ics",
			PropStat: struct {
				Status string `xml:"d:status"`
				Prop   struct {
					CalendarData string `xml:"cal:calendar-data"`
					GetETag      string `xml:"d:getetag"`
				} `xml:"d:prop"`
			}{
				Status: "HTTP/1.1 200 OK",
				Prop: struct {
					CalendarData string `xml:"cal:calendar-data"`
					GetETag      string `xml:"d:getetag"`
				}{
					CalendarData: string(data),
					GetETag:      etag,
				},
			},
		})
	}

	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	xml.NewEncoder(w).Encode(ms)
}

func (app *App) handleCalDAVPut(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) < 3 {
		http.Error(w, "bad path", 400)
		return
	}
	uid, cal, fname := parts[0], parts[1], parts[2]
	if !validEventFilename(fname) {
		http.Error(w, "invalid event path", http.StatusBadRequest)
		return
	}
	if !strings.HasSuffix(fname, ".ics") {
		http.Error(w, "expected .ics", 400)
		return
	}
	body, _ := io.ReadAll(r.Body)
	if len(body) == 0 {
		http.Error(w, "empty body", 400)
		return
	}
	if app.db != nil {
		event, err := app.parseICSBytes(body, strings.TrimSuffix(fname, ".ics"), cal)
		if err != nil {
			http.Error(w, "invalid calendar data", 400)
			return
		}
		event.CalendarID = app.resolveCalID(uid, cal)
		if err := app.writeEvent(uid, event); err != nil {
			http.Error(w, "write failed", 500)
			return
		}
	} else {
		path := filepath.Join(app.calendarDir(uid, cal), fname)
		if err := safefile.Write(path, body); err != nil {
			http.Error(w, "write failed", 500)
			return
		}
	}
	app.recordActivity(r.Context(), uid, "event_updated", "updated event "+strings.TrimSuffix(fname, ".ics"), "event", strings.TrimSuffix(fname, ".ics"), strings.TrimSuffix(fname, ".ics"), "dav")
	w.WriteHeader(http.StatusCreated)
}

func (app *App) handleCalDAVGet(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) < 3 {
		http.Error(w, "bad path", 400)
		return
	}
	uid, cal, fname := parts[0], parts[1], parts[2]
	if !validEventFilename(fname) {
		http.Error(w, "invalid event path", http.StatusBadRequest)
		return
	}
	var data []byte
	var err error
	if app.db != nil {
		event, _, findErr := app.findEvent(uid, strings.TrimSuffix(fname, ".ics"))
		err = findErr
		if err == nil {
			data = []byte(app.toICS(event))
		}
	} else {
		data, err = os.ReadFile(filepath.Join(app.calendarDir(uid, cal), fname))
	}
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	w.Header().Set("Content-Type", ical.MIMEType)
	w.Write(data)
}

func (app *App) handleCalDAVDelete(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) < 3 {
		http.Error(w, "bad path", 400)
		return
	}
	uid, cal, fname := parts[0], parts[1], parts[2]
	if !validEventFilename(fname) {
		http.Error(w, "invalid event path", http.StatusBadRequest)
		return
	}
	if app.db != nil {
		app.removeEventFile(uid, cal, strings.TrimSuffix(fname, ".ics"))
	} else {
		os.Remove(filepath.Join(app.calendarDir(uid, cal), fname))
	}
	app.recordActivity(r.Context(), uid, "event_deleted", "deleted event "+strings.TrimSuffix(fname, ".ics"), "event", strings.TrimSuffix(fname, ".ics"), strings.TrimSuffix(fname, ".ics"), "dav")
	w.WriteHeader(http.StatusNoContent)
}

// ── helpers ──

func (app *App) writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
