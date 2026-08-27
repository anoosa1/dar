package files

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/anoosa/dar/internal/apps"
)

func filesAssistantText(t *testing.T, app *App, ctx context.Context, uid, name, args string) string {
	t.Helper()
	result, err := app.CallAssistantTool(ctx, uid, name, json.RawMessage(args))
	if err != nil {
		t.Fatalf("%s: %v", name, err)
	}
	if len(result.Content) == 0 {
		t.Fatalf("%s returned no content", name)
	}
	return result.Content[0].Text
}

func TestFilesAssistantFullWorkflow(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	uid := "assistant-user"
	filesAssistantText(t, app, ctx, uid, "create_folder", `{"path":"/Assistant/Nested"}`)
	filesAssistantText(t, app, ctx, uid, "write_text", `{"path":"/Assistant/Nested/one.txt","content":"searchable assistant content"}`)
	for _, tc := range []struct {
		name string
		args string
	}{
		{"list", `{"path":"/Assistant/Nested"}`},
		{"search", `{"q":"one.txt"}`},
		{"read", `{"path":"/Assistant/Nested/one.txt"}`},
	} {
		filesAssistantText(t, app, ctx, uid, tc.name, tc.args)
	}
	filesAssistantText(t, app, ctx, uid, "favorite", `{"path":"/Assistant/Nested/one.txt","favorite":true}`)
	filesAssistantText(t, app, ctx, uid, "favorite", `{"path":"/Assistant/Nested/one.txt","favorite":true}`)
	filesAssistantText(t, app, ctx, uid, "favorite", `{"path":"/Assistant/Nested/one.txt","favorite":false}`)

	rec := filesCall(t, app, app.handleTags, http.MethodPost, "/api/files/tags", []byte(`{"name":"Assistant"}`), uid)
	var tag Tag
	if err := json.Unmarshal(rec.Body.Bytes(), &tag); err != nil {
		t.Fatal(err)
	}
	filesAssistantText(t, app, ctx, uid, "tag", `{"path":"/Assistant/Nested/one.txt","tag_id":"`+tag.ID+`","action":"add"}`)
	filesAssistantText(t, app, ctx, uid, "tag", `{"path":"/Assistant/Nested/one.txt","tag_id":"`+tag.ID+`","action":"remove"}`)
	filesAssistantText(t, app, ctx, uid, "copy", `{"from":"/Assistant/Nested/one.txt","to":"/Assistant/two.txt"}`)
	filesAssistantText(t, app, ctx, uid, "move", `{"from":"/Assistant/two.txt","to":"/Assistant/moved.txt"}`)
	filesAssistantText(t, app, ctx, uid, "rename", `{"path":"/Assistant/moved.txt","name":"final.txt"}`)
	filesAssistantText(t, app, ctx, uid, "create_symlink", `{"path":"/Assistant/link.txt","target":"final.txt"}`)
	filesAssistantText(t, app, ctx, uid, "share", `{"path":"/Assistant/final.txt"}`)

	filesAssistantText(t, app, ctx, uid, "trash", `{"path":"/Assistant/final.txt"}`)
	records, err := app.trashRecords(ctx, uid)
	if err != nil || len(records) != 1 {
		t.Fatalf("trash records=%v err=%v", records, err)
	}
	filesAssistantText(t, app, ctx, uid, "restore", `{"id":"`+records[0].ID+`"}`)
	filesAssistantText(t, app, ctx, uid, "trash", `{"path":"/Assistant/final.txt"}`)
	records, _ = app.trashRecords(ctx, uid)
	filesAssistantText(t, app, ctx, uid, "delete_permanently", `{"id":"`+records[0].ID+`"}`)

	for _, tc := range []struct {
		name string
		args string
	}{
		{"write_text", `{}`},
		{"create_folder", `{}`},
		{"rename", `{"path":"/a","name":"../bad"}`},
		{"move", `{}`},
		{"copy", `{}`},
		{"tag", `{}`},
		{"unknown", `{}`},
	} {
		if _, err := app.CallAssistantTool(ctx, uid, tc.name, json.RawMessage(tc.args)); err == nil {
			t.Errorf("%s unexpectedly succeeded", tc.name)
		}
	}
	if len(app.AssistantTools()) != len(fileAssistantTools) || apps.AssistantRead == "" {
		t.Fatal("Files assistant manifest changed")
	}
}
