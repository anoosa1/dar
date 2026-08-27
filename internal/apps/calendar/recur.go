package calendar

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/safefile"
	"github.com/teambition/rrule-go"
)

const maxRecurrenceRange = int64(5 * 366 * 24 * 60 * 60)
const maxRecurrenceInstances = 5000

func validEventID(id string) bool {
	id = strings.TrimSpace(id)
	if id == "" || id == "." || id == ".." || len(id) > 240 || filepath.Base(id) != id || strings.ContainsAny(id, `/\`) {
		return false
	}
	for _, r := range id {
		if r < 0x20 || r == 0x7f {
			return false
		}
	}
	return true
}

func validEventFilename(name string) bool {
	return strings.HasSuffix(name, ".ics") && validEventID(strings.TrimSuffix(name, ".ics"))
}

// masterIDFrom returns the stored ICS id for an event id that may be an
// expanded instance id of the form "master@recurrenceUnix".
func (app *App) masterIDFrom(id string) string {
	if i := strings.LastIndex(id, "@"); i > 0 {
		if _, err := strconv.ParseInt(id[i+1:], 10, 64); err == nil {
			return id[:i]
		}
	}
	return id
}

// expandInRange turns master events (including RRULE series) into concrete
// instances overlapping [rangeStart, rangeEnd). Non-recurring events pass through.
func (app *App) expandInRange(events []Event, rangeStart, rangeEnd int64) []Event {
	if rangeStart == 0 && rangeEnd == 0 {
		// No range: return masters only (search/export style).
		return events
	}
	if rangeEnd <= rangeStart {
		return []Event{}
	}
	if rangeEnd-rangeStart > maxRecurrenceRange {
		rangeEnd = rangeStart + maxRecurrenceRange
	}
	var out []Event
	for _, e := range events {
		if len(out) >= maxRecurrenceInstances {
			break
		}
		if e.RRule == "" {
			if e.Start < rangeEnd && e.End > rangeStart {
				out = append(out, e)
			}
			continue
		}
		instances := app.expandRecurring(e, rangeStart, rangeEnd)
		remaining := maxRecurrenceInstances - len(out)
		if len(instances) > remaining {
			instances = instances[:remaining]
		}
		out = append(out, instances...)
	}
	return out
}

func (app *App) expandRecurring(master Event, rangeStart, rangeEnd int64) []Event {
	ropt, err := rrule.StrToROption(master.RRule)
	if err != nil || ropt == nil {
		if master.Start < rangeEnd && master.End > rangeStart {
			return []Event{master}
		}
		return nil
	}
	// Dtstart must match the master event start.
	start := time.Unix(master.Start, 0)
	if master.AllDay {
		start = time.Unix(master.Start, 0).In(time.Local)
		start = time.Date(start.Year(), start.Month(), start.Day(), 0, 0, 0, 0, time.Local)
	} else {
		start = start.UTC()
	}
	ropt.Dtstart = start
	rule, err := rrule.NewRRule(*ropt)
	if err != nil {
		if master.Start < rangeEnd && master.End > rangeStart {
			return []Event{master}
		}
		return nil
	}

	dur := master.End - master.Start
	if dur <= 0 {
		if master.AllDay {
			dur = 86400
		} else {
			dur = 3600
		}
	}

	// Include occurrences that start slightly before the window but still overlap.
	// Between filters by occurrence start; pad start by duration.
	after := time.Unix(rangeStart-dur, 0)
	before := time.Unix(rangeEnd, 0)
	ex := make(map[int64]bool, len(master.ExDates))
	for _, x := range master.ExDates {
		ex[app.normalizeExDate(x, master.AllDay)] = true
	}

	var out []Event
	next := rule.Iterator()
	for scanned := 0; scanned < 20000 && len(out) < 1000; scanned++ {
		t, ok := next()
		if !ok || t.After(before) {
			break
		}
		if t.Before(after) {
			continue
		}
		st := t.Unix()
		if master.AllDay {
			// Normalize to local midnight unix for stable ids / exdates.
			loc := t.In(time.Local)
			st = time.Date(loc.Year(), loc.Month(), loc.Day(), 0, 0, 0, 0, time.Local).Unix()
		}
		if ex[app.normalizeExDate(st, master.AllDay)] {
			continue
		}
		en := st + dur
		if st >= rangeEnd || en <= rangeStart {
			continue
		}
		inst := master
		inst.MasterID = master.ID
		inst.RecurrenceID = st
		inst.Start = st
		inst.End = en
		// Synthetic id so the UI can address a single instance.
		inst.ID = fmt.Sprintf("%s@%d", master.ID, st)
		// Keep rrule on instance for "Repeats" badge / edit-scope UI.
		out = append(out, inst)
	}
	return out
}

func (app *App) normalizeExDate(ts int64, allDay bool) int64 {
	if !allDay {
		return ts
	}
	t := time.Unix(ts, 0).In(time.Local)
	return time.Date(t.Year(), t.Month(), t.Day(), 0, 0, 0, 0, time.Local).Unix()
}

func (app *App) hasExDate(list []int64, ts int64, allDay bool) bool {
	n := app.normalizeExDate(ts, allDay)
	for _, x := range list {
		if app.normalizeExDate(x, allDay) == n {
			return true
		}
	}
	return false
}

func (app *App) appendExDate(list []int64, ts int64, allDay bool) []int64 {
	n := app.normalizeExDate(ts, allDay)
	if app.hasExDate(list, n, allDay) {
		return list
	}
	return append(list, n)
}

// rruleWithUntil returns RRULE string with UNTIL set to the instant before untilTs.
// untilTs is the first occurrence to exclude (this instance for "future" split).
func (app *App) rruleWithUntil(rruleStr string, untilTs int64, allDay bool) string {
	ropt, err := rrule.StrToROption(rruleStr)
	if err != nil || ropt == nil {
		return rruleStr
	}
	// UNTIL is the last included occurrence end; use day before instance for all-day,
	// or one second before for timed events.
	var until time.Time
	if allDay {
		t := time.Unix(untilTs, 0).In(time.Local).AddDate(0, 0, -1)
		until = time.Date(t.Year(), t.Month(), t.Day(), 23, 59, 59, 0, time.UTC)
	} else {
		until = time.Unix(untilTs, 0).UTC().Add(-time.Second)
	}
	ropt.Until = until
	ropt.Count = 0
	return ropt.RRuleString()
}

func (app *App) writeEvent(uid string, e Event) error {
	if !validEventID(e.ID) {
		return fmt.Errorf("invalid event id")
	}
	e.CalendarID = app.resolveCalID(uid, e.CalendarID)
	var err error
	if app.db != nil {
		err = app.sqlWriteEvent(uid, e)
	} else {
		path := filepath.Join(app.calendarDir(uid, e.CalendarID), e.ID+".ics")
		err = safefile.Write(path, []byte(app.toICS(e)))
	}
	if err == nil {
		app.replaceEventSearch(uid, e)
	}
	return err
}

func (app *App) removeEventFile(uid, cal, id string) {
	if !validEventID(id) {
		return
	}
	if app.db != nil {
		_ = app.sqlDeleteEvent(uid, app.resolveCalID(uid, cal), id)
	} else {
		_ = os.Remove(filepath.Join(app.calendarDir(uid, cal), id+".ics"))
	}
	app.deleteEventSearch(uid, cal, id)
}
