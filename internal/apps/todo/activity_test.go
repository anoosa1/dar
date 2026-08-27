package todo

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"testing"

	"github.com/anoosa/dar/internal/testutil"
)

func TestTodoActivityRecordingBranches(t *testing.T) {
	app := testApp(t)
	ctx := context.Background()
	owner := todoUser(t, app, "owner")
	member := todoUser(t, app, "member")
	rec := todoCall(t, app.handleLists, http.MethodPost, "/api/todo/lists", TaskList{Name: "Shared", Color: "#123456"}, owner)
	var list TaskList
	if rec.Code != http.StatusOK || json.Unmarshal(rec.Body.Bytes(), &list) != nil {
		t.Fatalf("create list: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleList, http.MethodPost, "/api/todo/lists/"+list.ID+"/share", map[string]any{"email": member.Email, "permission": "write"}, owner)
	if rec.Code != http.StatusOK {
		t.Fatalf("share list: %d %s", rec.Code, rec.Body.String())
	}
	sink := &testutil.ActivitySink{}
	app.registry = testutil.RegistryWithActivity(sink, app)
	app.recordActivity(ctx, owner.ID, list.ID, "created", "Created task", "task", "t1", "Task")
	app.recordActivityOrigin(ctx, owner.ID, list.ID, "updated", "Updated task", "task", "t1", "Task", "caldav")
	app.recordActivity(ctx, "", list.ID, "ignored", "Ignored", "task", "t2", "Task")
	sink.Err = errors.New("recording failed")
	app.recordActivity(ctx, owner.ID, list.ID, "deleted", "Deleted task", "task", "t1", "Task")
	if len(sink.Events) != 3 || len(sink.Events[0].RecipientIDs) < 2 {
		t.Fatalf("events=%+v", sink.Events)
	}
}
