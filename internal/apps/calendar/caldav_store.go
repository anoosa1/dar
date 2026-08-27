package calendar

import (
	"fmt"
	"strings"

	"github.com/anoosa/dar/internal/caldav"
)

// The Calendar app exposes VEVENTs through the shared CalDAV service. Its
// existing SQLite event representation stays authoritative for browser and
// protocol writes alike.
func (a *App) Collections(uid string) ([]caldav.Collection, error) {
	metas := a.listCalendarMetas(uid)
	out := make([]caldav.Collection, 0, len(metas))
	for _, m := range metas {
		out = append(out, caldav.Collection{ID: m.ID, Name: m.Name, Color: m.Color, Permission: "owner"})
	}
	return out, nil
}
func (a *App) CreateCollection(uid, name string) (caldav.Collection, error) {
	id := a.sanitizeCalID(name)
	if id == "" {
		return caldav.Collection{}, fmt.Errorf("invalid calendar name")
	}
	m := CalendarMeta{ID: id, Name: name, Color: calendarTheme.Pink, Visible: true}
	if err := a.saveMeta(uid, m); err != nil {
		return caldav.Collection{}, err
	}
	return caldav.Collection{ID: m.ID, Name: m.Name, Color: m.Color, Permission: "owner"}, nil
}
func (a *App) Objects(uid, collection string) ([]caldav.Object, error) {
	events := a.listEventsInCalendar(uid, collection)
	out := make([]caldav.Object, 0, len(events))
	for _, e := range events {
		out = append(out, caldav.Object{Href: e.ID + ".ics", Data: a.toICS(e), ETag: fmt.Sprintf(`"%x"`, e.Start+e.End)})
	}
	return out, nil
}
func (a *App) Object(uid, collection, href string) (caldav.Object, error) {
	e, cal, err := a.findEvent(uid, strings.TrimSuffix(href, ".ics"))
	if err != nil || cal != collection {
		return caldav.Object{}, fmt.Errorf("not found")
	}
	return caldav.Object{Href: href, Data: a.toICS(e), ETag: fmt.Sprintf(`"%x"`, e.Start+e.End)}, nil
}
func (a *App) PutObject(uid, collection, href, data string) (caldav.Object, error) {
	e, err := a.parseICSBytes([]byte(data), strings.TrimSuffix(href, ".ics"), collection)
	if err != nil {
		return caldav.Object{}, err
	}
	e.CalendarID = a.resolveCalID(uid, collection)
	if err := a.writeEvent(uid, e); err != nil {
		return caldav.Object{}, err
	}
	return caldav.Object{Href: href, Data: a.toICS(e), ETag: fmt.Sprintf(`"%x"`, e.Start+e.End)}, nil
}
func (a *App) DeleteObject(uid, collection, href string) error {
	a.removeEventFile(uid, collection, strings.TrimSuffix(href, ".ics"))
	return nil
}

var _ caldav.Store = (*App)(nil)
