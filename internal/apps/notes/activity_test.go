package notes

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestNotesActivityRecordingBranches(t *testing.T) {
	app, _ := testNotesApp(t)
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(context.Background(), "alice", "created", "Created note", "note", "n1", "Note")
	app.recordActivity(context.Background(), "", "ignored", "Ignored", "note", "n2", "Note")
	sink.Err = errors.New("recording failed")
	app.recordActivity(context.Background(), "alice", "updated", "Updated note", "note", "n1", "Note")
	if len(sink.Events) != 2 {
		t.Fatalf("events=%+v", sink.Events)
	}
}
