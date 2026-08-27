package news

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestNewsActivityRecordingBranches(t *testing.T) {
	app := testApp(t)
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(context.Background(), "alice", "created", "Created feed", "feed", "f1", "Feed", "api")
	app.recordActivity(context.Background(), "", "ignored", "Ignored", "feed", "f2", "Feed", "api")
	app.recordActivity(context.Background(), "alice", "refreshed", "Refreshed feed", "feed", "f1", "Feed", "worker")
	sink.Err = errors.New("recording failed")
	app.recordActivity(context.Background(), "alice", "updated", "Updated feed", "feed", "f1", "Feed", "api")
	if len(sink.Events) != 3 || sink.Events[1].ActorKind != "system" {
		t.Fatalf("events=%+v", sink.Events)
	}
}
