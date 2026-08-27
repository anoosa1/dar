package files

import (
	"context"
	"errors"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestFilesActivityRecipientsNamesAndOrigins(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/SharedFolder"); err != nil {
		t.Fatal(err)
	}
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.logActivity("alice", "/SharedFolder/a b.txt", "a b.txt", "created")
	app.logActivityOrigin("alice", "worker", "/outside.txt", "outside.txt", "indexed")
	app.logActivityFor("", "guest", "public", "/SharedFolder/upload.txt", "upload.txt", "uploaded", []string{"alice"})
	app.logActivityFor("", "guest", "public", "/", "ignored", "ignored", nil)
	sink.Err = errors.New("recording failed")
	app.logActivity("alice", "/failure.txt", "failure.txt", "updated")
	if len(sink.Events) != 4 || len(sink.Events[0].RecipientIDs) != 1 || sink.Events[2].ActorName != "Guest" {
		t.Fatalf("events=%+v", sink.Events)
	}
}
