package email

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestEmailActivityRecordingBranches(t *testing.T) {
	app := testApp(t, testKey(t, "activity"))
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(context.Background(), "alice", "sent", "Sent mail", "message", "m1", "Message", "api")
	app.recordActivity(context.Background(), "", "ignored", "Ignored", "message", "m2", "Message", "api")
	app.recordActivity(context.Background(), "alice", "received", "Received mail", "message", "m3", "Message", "worker")
	sink.Err = errors.New("recording failed")
	app.recordActivity(context.Background(), "alice", "updated", "Updated mail", "message", "m1", "Message", "background")
	if len(sink.Events) != 3 || sink.Events[1].ActorKind != "system" {
		t.Fatalf("events=%+v", sink.Events)
	}
}
