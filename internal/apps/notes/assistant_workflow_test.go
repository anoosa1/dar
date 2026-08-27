package notes

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func assistantText(t *testing.T, app *App, ctx context.Context, userID, name, arguments string) string {
	t.Helper()
	result, err := app.CallAssistantTool(ctx, userID, name, json.RawMessage(arguments))
	if err != nil {
		t.Fatalf("%s: %v", name, err)
	}
	if len(result.Content) == 0 {
		t.Fatal("assistant result has no content")
	}
	return result.Content[0].Text
}

func TestNotesAssistantFullWorkflow(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	uid := "assistant-user"

	assistantText(t, app, ctx, uid, "create_note_category", `{"name":"Projects"}`)
	createdText := assistantText(t, app, ctx, uid, "create_note", `{"title":"Plan","content":"Initial body","category":"Projects","favorite":true}`)
	var created Note
	if err := json.Unmarshal([]byte(createdText), &created); err != nil {
		t.Fatal(err)
	}
	if created.ID == 0 {
		t.Fatalf("created note=%+v", created)
	}
	id := fmt.Sprint(created.ID)

	for _, tc := range []struct {
		name string
		args string
	}{
		{"list_notes", `{"q":"Initial","category":"Projects","summary":1}`},
		{"read_note", `{"id":` + id + `}`},
		{"list_note_categories", `{}`},
		{"get_notes_preferences", `{}`},
	} {
		if text := assistantText(t, app, ctx, uid, tc.name, tc.args); text == "" {
			t.Errorf("%s returned empty content", tc.name)
		}
	}
	assistantText(t, app, ctx, uid, "update_note", `{"id":`+id+`,"title":"Updated","content":"Changed body","category":"Projects","force":true}`)
	assistantText(t, app, ctx, uid, "rename_note_category", `{"from":"Projects","to":"Archive"}`)
	versions := assistantText(t, app, ctx, uid, "list_note_versions", `{"id":`+id+`}`)
	if !strings.Contains(versions, "versions") {
		t.Fatalf("versions=%s", versions)
	}
	_, _ = app.CallAssistantTool(ctx, uid, "undo_note", json.RawMessage(`{"id":`+id+`}`))
	assistantText(t, app, ctx, uid, "set_notes_preferences", `{"notesPath":"/Notes","fileSuffix":".md","editorMode":"rich"}`)

	for _, tc := range []struct {
		name string
		args string
	}{
		{"read_note_attachment", `{"id":` + id + `,"path":"missing.png"}`},
		{"upload_note_attachment", `{"id":` + id + `,"filename":"bad.png","data_base64":"!"}`},
		{"unknown", `{}`},
	} {
		if _, err := app.CallAssistantTool(ctx, uid, tc.name, json.RawMessage(tc.args)); err == nil {
			t.Errorf("%s unexpectedly succeeded", tc.name)
		}
	}
	if _, err := app.CallAssistantTool(ctx, uid, "read_note", json.RawMessage(`{`)); err == nil {
		t.Fatal("invalid arguments accepted")
	}
	assistantText(t, app, ctx, uid, "delete_note", `{"id":`+id+`}`)
	assistantText(t, app, ctx, uid, "delete_note_category", `{"name":"Archive","force":true}`)
}
