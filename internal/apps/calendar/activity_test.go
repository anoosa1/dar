package calendar

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestCalendarActivityRecordingBranches(t *testing.T) {
	app := newSQLTestApp(t)
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(context.Background(), "alice", "created", "Created event", "event", "e1", "Event", "")
	app.recordActivity(context.Background(), "", "ignored", "Ignored", "event", "e2", "Event", "api")
	sink.Err = errors.New("recording failed")
	app.recordActivity(context.Background(), "alice", "updated", "Updated event", "event", "e1", "Event", "worker")
	if len(sink.Events) != 2 || sink.Events[0].Origin != "api" {
		t.Fatalf("events=%+v", sink.Events)
	}
}
