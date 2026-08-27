package calendar

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/preferences"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{
		{Name: "list_calendars", Description: "List the user's calendars", InputSchema: calendarSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "list_events", Description: "List events, optionally by calendar and Unix time range", InputSchema: calendarSchema(`{"calendar":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"}}`), Effect: apps.AssistantRead},
		{Name: "search_events", Description: "Search calendar event titles, descriptions, and locations", InputSchema: calendarSchema(`{"q":{"type":"string"}}`, "q"), Effect: apps.AssistantRead},
		{Name: "read_event", Description: "Read one calendar event", InputSchema: calendarSchema(`{"id":{"type":"string"},"calendar":{"type":"string"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "create_calendar", Description: "Create a calendar", InputSchema: calendarSchema(`{"name":{"type":"string"},"color":{"type":"string"},"id":{"type":"string"}}`, "name"), Effect: apps.AssistantSafeWrite},
		{Name: "update_calendar", Description: "Update a calendar's name, color, visibility, or order", InputSchema: calendarSchema(`{"id":{"type":"string"},"name":{"type":"string"},"color":{"type":"string"},"visible":{"type":"boolean"},"order":{"type":"integer"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_calendar", Description: "Permanently delete a calendar and its events", InputSchema: calendarSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "create_event", Description: "Create a calendar event", InputSchema: calendarSchema(`{"title":{"type":"string"},"description":{"type":"string"},"location":{"type":"string"},"calendar":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"},"all_day":{"type":"boolean"},"rrule":{"type":"string"}}`, "title", "start"), Effect: apps.AssistantSafeWrite},
		{Name: "update_event", Description: "Update a calendar event", InputSchema: calendarSchema(`{"id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"location":{"type":"string"},"calendar":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"},"all_day":{"type":"boolean"},"rrule":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_event", Description: "Delete a calendar event or recurrence", InputSchema: calendarSchema(`{"id":{"type":"string"},"calendar":{"type":"string"},"scope":{"type":"string","enum":["this","future","all"]},"recurrence_id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "list_subscriptions", Description: "List subscribed remote calendars", InputSchema: calendarSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "add_subscription", Description: "Subscribe to a remote WebCal or ICS calendar", InputSchema: calendarSchema(`{"name":{"type":"string"},"url":{"type":"string"},"color":{"type":"string"}}`, "name", "url"), Effect: apps.AssistantExternal},
		{Name: "refresh_subscription", Description: "Fetch a subscribed calendar now", InputSchema: calendarSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantExternal},
		{Name: "remove_subscription", Description: "Remove a subscribed calendar", InputSchema: calendarSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "get_calendar_preferences", Description: "Read Calendar display and reminder preferences", InputSchema: calendarSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_calendar_preferences", Description: "Update Calendar display and reminder preferences", InputSchema: calendarSchema(`{"timezone":{"type":"string"},"birthday_calendar":{"type":"boolean"},"show_weekends":{"type":"boolean"},"show_week_numbers":{"type":"boolean"},"limit_month_events":{"type":"boolean"},"slot_minutes":{"type":"integer","enum":[5,10,15,20,30,60]},"simple_editor":{"type":"boolean"}}`), Effect: apps.AssistantSafeWrite},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	switch name {
	case "list_calendars":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCalendars, http.MethodGet, "/api/calendar/calendars", arguments)
	case "list_events":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodGet, "/api/calendar/list", arguments)
	case "search_events":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSearch, http.MethodGet, "/api/calendar/search", arguments)
	case "read_event":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleGet, http.MethodGet, "/api/calendar/get", arguments)
	case "create_calendar":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCalendars, http.MethodPost, "/api/calendar/calendars", arguments)
	case "update_calendar":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", arguments)
	case "create_event":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCreate, http.MethodPost, "/api/calendar/create", arguments)
	case "update_event":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleUpdate, http.MethodPost, "/api/calendar/update", arguments)
	case "list_subscriptions":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSubscriptions, http.MethodGet, "/api/calendar/subscriptions", arguments)
	case "add_subscription":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", arguments)
	case "get_calendar_preferences":
		return a.callCalendarPreferences(ctx, userID, nil)
	case "set_calendar_preferences":
		return a.callCalendarPreferences(ctx, userID, arguments)
	}
	var input map[string]any
	if err := json.Unmarshal(arguments, &input); err != nil {
		return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
	}
	q := url.Values{}
	for key, value := range input {
		q.Set(key, fmt.Sprint(value))
	}
	switch name {
	case "delete_calendar":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete?"+q.Encode(), nil)
	case "delete_event":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleDelete, http.MethodPost, "/api/calendar/delete?"+q.Encode(), nil)
	case "refresh_subscription":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh?"+q.Encode(), nil)
	case "remove_subscription":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSubscriptions, http.MethodDelete, "/api/calendar/subscriptions", arguments)
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown calendar tool %q", name)
	}
}

type assistantCalendarPreferences struct {
	Timezone         string `json:"timezone"`
	BirthdayCalendar bool   `json:"birthday_calendar"`
	ShowWeekends     bool   `json:"show_weekends"`
	ShowWeekNumbers  bool   `json:"show_week_numbers"`
	LimitMonthEvents bool   `json:"limit_month_events"`
	SlotMinutes      int    `json:"slot_minutes"`
	SimpleEditor     bool   `json:"simple_editor"`
}

func (a *App) callCalendarPreferences(ctx context.Context, userID string, update json.RawMessage) (apps.AssistantResult, error) {
	value := assistantCalendarPreferences{Timezone: "automatic", BirthdayCalendar: true, ShowWeekends: true, LimitMonthEvents: true, SlotMinutes: 30, SimpleEditor: true}
	if a.db == nil {
		return apps.AssistantResult{}, fmt.Errorf("calendar database unavailable")
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return apps.AssistantResult{}, err
	}
	defer release()
	if _, err := preferences.GetJSON(conn, userID, "calendar", "settings", &value); err != nil {
		return apps.AssistantResult{}, err
	}
	if update != nil {
		if err := json.Unmarshal(update, &value); err != nil {
			return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
		}
		value.Timezone = strings.TrimSpace(value.Timezone)
		if value.Timezone == "" {
			value.Timezone = "automatic"
		}
		if value.Timezone != "automatic" {
			if _, err := time.LoadLocation(value.Timezone); err != nil {
				return apps.AssistantResult{}, fmt.Errorf("invalid timezone")
			}
		}
		if value.SlotMinutes != 5 && value.SlotMinutes != 10 && value.SlotMinutes != 15 && value.SlotMinutes != 20 && value.SlotMinutes != 30 && value.SlotMinutes != 60 {
			return apps.AssistantResult{}, fmt.Errorf("invalid slot_minutes")
		}
		if err := preferences.SetJSON(conn, userID, "calendar", "settings", value); err != nil {
			return apps.AssistantResult{}, err
		}
	}
	data, _ := json.Marshal(value)
	return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(data)}}}, nil
}

func calendarSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
