package calendar

import (
	"strings"
	"testing"
)

func TestCalendarPageContainsAppSettings(t *testing.T) {
	html := calendarHTML()
	for _, want := range []string{
		`id="calendar-settings-modal"`,
		`id="calendar-menu-modal"`,
		`calendar-menu-delete-confirm`,
		`refreshCalendarSubscription`,
		`id="detail-pane-resizer"`,
		`Resize event details`,
		`resizeDetailPane`,
		`function eventColor(event)`,
		`function eventTextColor(event)`,
		`background:' + esc(eventColor(e))`,
		`color:' + esc(eventTextColor(e))`,
		`max-h-56 overflow-y-auto overflow-x-hidden whitespace-pre-wrap break-words`,
		`noteText(e.description)`,
		`onclick="openSettings()"`,
		`/api/settings/calendar`,
		`calendar-setting-timezone`,
		`calendar-setting-slot-minutes`,
		`calendar-import-input`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("calendar HTML does not contain %q", want)
		}
	}
	if strings.Contains(html, `href="/settings"`) {
		t.Fatal("Calendar settings entry still routes to global settings")
	}
}
