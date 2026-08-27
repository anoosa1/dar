package contacts

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestContactsActivityRecordingBranches(t *testing.T) {
	app := preferencesTestApp(t)
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(context.Background(), "alice", "created", "Created contact", "contact", "c1", "Contact", "api")
	app.recordActivity(context.Background(), "", "ignored", "Ignored", "contact", "c2", "Contact", "api")
	sink.Err = errors.New("recording failed")
	app.recordActivity(context.Background(), "alice", "updated", "Updated contact", "contact", "c1", "Contact", "api")
	if len(sink.Events) != 2 {
		t.Fatalf("events=%+v", sink.Events)
	}
}
