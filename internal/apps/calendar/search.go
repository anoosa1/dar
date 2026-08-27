package calendar

import (
	"context"
	"log"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

func (app *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	for _, event := range app.listAllEvents(uid) {
		if err := sink.AddSearchDocument(ctx, app.eventSearchDocument(uid, event)); err != nil {
			return err
		}
	}
	return nil
}

func (app *App) eventSearchDocument(uid string, event Event) apps.SearchDocument {
	meta := app.loadMeta(uid, event.CalendarID)
	objectID := event.CalendarID + ":" + event.ID
	subtitle := meta.Name
	if event.Start > 0 {
		subtitle += " · " + time.Unix(event.Start, 0).Format("Jan 2, 2006 3:04 PM")
	}
	modifiedAt := event.Start
	if modifiedAt == 0 {
		modifiedAt = time.Now().Unix()
	}
	return apps.SearchDocument{
		ObjectID: objectID, ChunkID: "event", Title: firstCalendarTitle(event.Title),
		Subtitle: subtitle, Keywords: strings.Join(append(event.Categories, meta.Name, event.Location), " "),
		Body: event.Description, URL: "/calendar", Anchor: "event-" + event.ID,
		ModifiedAt: modifiedAt,
	}
}

func (app *App) replaceEventSearch(uid string, event Event) {
	if app.search == nil {
		return
	}
	doc := app.eventSearchDocument(uid, event)
	if err := app.search.ReplaceSearchObject(context.Background(), uid, doc.ObjectID, []apps.SearchDocument{doc}); err != nil {
		log.Printf("calendar: update search index: %v", err)
		app.markSearchDirty()
	}
}

func (app *App) deleteEventSearch(uid, calendarID, eventID string) {
	if app.search == nil {
		return
	}
	objectID := app.resolveCalID(uid, calendarID) + ":" + eventID
	if err := app.search.DeleteSearchObject(context.Background(), uid, objectID); err != nil {
		log.Printf("calendar: delete search index: %v", err)
		app.markSearchDirty()
	}
}

func firstCalendarTitle(value string) string {
	if strings.TrimSpace(value) == "" {
		return "Untitled event"
	}
	return value
}

func (app *App) markSearchDirty() {
	if app.search != nil {
		app.search.MarkSearchDirty("calendar")
	}
}
