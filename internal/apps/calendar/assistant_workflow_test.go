package calendar

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strconv"
	"testing"
)

func calendarAssistantText(t *testing.T, app *App, ctx context.Context, userID, name, arguments string) string {
	t.Helper()
	result, err := app.CallAssistantTool(ctx, userID, name, json.RawMessage(arguments))
	if err != nil {
		t.Fatalf("%s: %v", name, err)
	}
	if len(result.Content) == 0 {
		t.Fatal("assistant result has no content")
	}
	return result.Content[0].Text
}

func TestCalendarAssistantFullWorkflow(t *testing.T) {
	a := newSQLTestApp(t)
	ctx := context.Background()
	uid := "assistant-user"

	calendarText := calendarAssistantText(t, a, ctx, uid, "create_calendar", `{"name":"Assistant calendar","color":"#123456","id":"assistant"}`)
	var calendar CalendarMeta
	if err := json.Unmarshal([]byte(calendarText), &calendar); err != nil {
		t.Fatal(err)
	}
	eventText := calendarAssistantText(t, a, ctx, uid, "create_event", `{"title":"Assistant event","description":"search target","location":"Room","calendar":"`+calendar.ID+`","start":1800000000,"end":1800003600}`)
	var event Event
	if err := json.Unmarshal([]byte(eventText), &event); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		name string
		args string
	}{
		{"list_calendars", `{}`},
		{"list_events", `{"calendar":"` + calendar.ID + `","start":1799990000,"end":1800010000}`},
		{"search_events", `{"q":"search"}`},
		{"read_event", `{"id":"` + event.ID + `"}`},
		{"get_calendar_preferences", `{}`},
	} {
		if text := calendarAssistantText(t, a, ctx, uid, tc.name, tc.args); text == "" {
			t.Errorf("%s returned empty content", tc.name)
		}
	}
	calendarAssistantText(t, a, ctx, uid, "update_calendar", `{"id":"`+calendar.ID+`","name":"Renamed calendar","visible":true,"order":2}`)
	calendarAssistantText(t, a, ctx, uid, "update_event", `{"id":"`+event.ID+`","title":"Updated event","calendar":"`+calendar.ID+`","start":1800000000,"end":1800007200}`)
	calendarAssistantText(t, a, ctx, uid, "set_calendar_preferences", `{"timezone":"America/Toronto","birthday_calendar":false,"show_weekends":false,"show_week_numbers":true,"limit_month_events":false,"slot_minutes":15,"simple_editor":false}`)
	for _, bad := range []string{`{`, `{"timezone":"Mars/Olympus","slot_minutes":30}`, `{"timezone":"UTC","slot_minutes":7}`} {
		if _, err := a.CallAssistantTool(ctx, uid, "set_calendar_preferences", json.RawMessage(bad)); err == nil {
			t.Errorf("invalid preferences accepted: %s", bad)
		}
	}

	feed := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:remote\r\nDTSTART:20260729T090000Z\r\nDTEND:20260729T100000Z\r\nSUMMARY:Remote\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"))
	}))
	defer feed.Close()
	subText := calendarAssistantText(t, a, ctx, uid, "add_subscription", `{"name":"Remote","url":"`+feed.URL+`","color":"#654321"}`)
	var subscription CalendarMeta
	if err := json.Unmarshal([]byte(subText), &subscription); err != nil {
		t.Fatal(err)
	}
	calendarAssistantText(t, a, ctx, uid, "list_subscriptions", `{}`)
	calendarAssistantText(t, a, ctx, uid, "refresh_subscription", `{"id":"`+subscription.ID+`"}`)
	calendarAssistantText(t, a, ctx, uid, "remove_subscription", `{"id":"`+subscription.ID+`"}`)

	calendarAssistantText(t, a, ctx, uid, "delete_event", `{"id":"`+event.ID+`","calendar":"`+calendar.ID+`","scope":"all","recurrence_id":`+strconv.FormatInt(event.RecurrenceID, 10)+`}`)
	calendarAssistantText(t, a, ctx, uid, "delete_calendar", `{"id":"`+calendar.ID+`"}`)
	if _, err := a.CallAssistantTool(ctx, uid, "unknown", json.RawMessage(`{}`)); err == nil {
		t.Fatal("unknown calendar tool accepted")
	}
	if _, err := a.CallAssistantTool(ctx, uid, "delete_event", json.RawMessage(`{`)); err == nil {
		t.Fatal("invalid calendar tool arguments accepted")
	}
}
