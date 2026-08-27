package todo

import (
	"context"
	"encoding/json"
	"testing"
)

func todoAssistantText(t *testing.T, app *App, ctx context.Context, userID, name, arguments string) string {
	t.Helper()
	result, err := app.CallAssistantTool(ctx, userID, name, json.RawMessage(arguments))
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Content) == 0 {
		t.Fatal("assistant result has no content")
	}
	return result.Content[0].Text
}

func TestTodoAssistantFullWorkflow(t *testing.T) {
	app := testApp(t)
	alice := todoUser(t, app, "assistant-alice")
	bob := todoUser(t, app, "assistant-bob")
	ctx := context.Background()

	listText := todoAssistantText(t, app, ctx, alice.ID, "create_task_list", `{"name":"Assistant list","color":"#123456"}`)
	var list TaskList
	if err := json.Unmarshal([]byte(listText), &list); err != nil {
		t.Fatal(err)
	}
	taskText := todoAssistantText(t, app, ctx, alice.ID, "create_task", `{"list_id":"`+list.ID+`","title":"Assistant task","notes":"search target","priority":4,"tags":["ai"]}`)
	var task Task
	if err := json.Unmarshal([]byte(taskText), &task); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		name string
		args string
	}{
		{"list_task_lists", `{}`},
		{"read_task_list", `{"id":"` + list.ID + `"}`},
		{"list_tasks", `{"list_id":"` + list.ID + `"}`},
		{"search_tasks", `{"q":"search"}`},
		{"read_task", `{"id":"` + task.ID + `"}`},
		{"get_todo_preferences", `{}`},
	} {
		if text := todoAssistantText(t, app, ctx, alice.ID, tc.name, tc.args); text == "" {
			t.Errorf("%s returned empty content", tc.name)
		}
	}
	todoAssistantText(t, app, ctx, alice.ID, "update_task_list", `{"id":"`+list.ID+`","name":"Renamed"}`)
	todoAssistantText(t, app, ctx, alice.ID, "update_task", `{"id":"`+task.ID+`","title":"Updated task","status":"IN-PROCESS","percent_complete":50}`)
	todoAssistantText(t, app, ctx, alice.ID, "complete_task", `{"id":"`+task.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "share_task_list", `{"id":"`+list.ID+`","email":"`+bob.Email+`","permission":"read"}`)
	todoAssistantText(t, app, ctx, alice.ID, "list_task_list_shares", `{"id":"`+list.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "unshare_task_list", `{"id":"`+list.ID+`","user_id":"`+bob.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "set_todo_preferences", `{"default_list_id":"`+list.ID+`","sort_mode":"priority","smart_collections":{}}`)

	todoAssistantText(t, app, ctx, alice.ID, "trash_task", `{"id":"`+task.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "restore_task", `{"id":"`+task.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "trash_task", `{"id":"`+task.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "delete_task_permanently", `{"id":"`+task.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "trash_task_list", `{"id":"`+list.ID+`"}`)
	todoAssistantText(t, app, ctx, alice.ID, "restore_task_list", `{"id":"`+list.ID+`"}`)

	for _, tc := range []struct {
		name string
		args string
	}{
		{"read_task_list", `{"id":"missing"}`},
		{"delete_task_permanently", `{"id":"missing"}`},
		{"unknown", `{}`},
	} {
		if _, err := app.CallAssistantTool(ctx, alice.ID, tc.name, json.RawMessage(tc.args)); err == nil {
			t.Errorf("%s unexpectedly succeeded", tc.name)
		}
	}
}
