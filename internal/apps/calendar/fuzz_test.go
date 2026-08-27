package calendar

import "testing"

func FuzzCalendarIdentifiersAndRecurrence(f *testing.F) {
	for _, seed := range []struct {
		id    string
		rule  string
		until int64
	}{
		{"event-1", "FREQ=DAILY;COUNT=3", 1_800_000_000},
		{"../escape", "FREQ=WEEKLY;BYDAY=MO,WE", 0},
		{"event.ics", "not-a-rule", -1},
	} {
		f.Add(seed.id, seed.rule, seed.until)
	}
	f.Fuzz(func(t *testing.T, id, rule string, until int64) {
		if len(id) > 4096 || len(rule) > 64<<10 {
			return
		}
		_ = validEventID(id)
		_ = validEventFilename(id)
		app := &App{}
		_ = app.rruleWithUntil(rule, until, false)
		_ = app.rruleWithUntil(rule, until, true)
	})
}
