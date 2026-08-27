package todo

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	taskFields := `{"id":{"type":"string"},"list_id":{"type":"string"},"parent_id":{"type":"string"},"title":{"type":"string"},"notes":{"type":"string"},"status":{"type":"string"},"percent_complete":{"type":"integer"},"priority":{"type":"integer"},"starred":{"type":"boolean"},"start_at":{"type":"integer"},"due_at":{"type":"integer"},"rrule":{"type":"string"},"is_habit":{"type":"boolean"},"tags":{"type":"array","items":{"type":"string"}}}`
	return []apps.AssistantTool{
		{Name: "list_task_lists", Description: "List task lists", InputSchema: todoSchema(`{"trash":{"type":"boolean"}}`), Effect: apps.AssistantRead},
		{Name: "read_task_list", Description: "Read one task list", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "list_tasks", Description: "List or search tasks by list, collection, or trash", InputSchema: todoSchema(`{"list_id":{"type":"string"},"collection":{"type":"string"},"trash":{"type":"boolean"}}`), Effect: apps.AssistantRead},
		{Name: "search_tasks", Description: "Search task titles, notes, and tags", InputSchema: todoSchema(`{"q":{"type":"string"}}`, "q"), Effect: apps.AssistantRead},
		{Name: "read_task", Description: "Read one task", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "create_task_list", Description: "Create a task list", InputSchema: todoSchema(`{"name":{"type":"string"},"color":{"type":"string"}}`, "name"), Effect: apps.AssistantSafeWrite},
		{Name: "update_task_list", Description: "Rename or recolor a task list", InputSchema: todoSchema(`{"id":{"type":"string"},"name":{"type":"string"},"color":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "trash_task_list", Description: "Move a task list to trash", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "restore_task_list", Description: "Restore a task list from trash", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "create_task", Description: "Create a task", InputSchema: todoSchema(taskFields, "title"), Effect: apps.AssistantSafeWrite},
		{Name: "update_task", Description: "Update a task", InputSchema: todoSchema(taskFields, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "complete_task", Description: "Mark a task occurrence complete", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "trash_task", Description: "Move a task to trash", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "restore_task", Description: "Restore a task from trash", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_task_permanently", Description: "Permanently remove a trashed task", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "list_task_list_shares", Description: "List a task list's shares", InputSchema: todoSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "share_task_list", Description: "Share a task list with a registered Dar user", InputSchema: todoSchema(`{"id":{"type":"string"},"email":{"type":"string"},"permission":{"type":"string","enum":["read","write"]}}`, "id", "email", "permission"), Effect: apps.AssistantExternal},
		{Name: "unshare_task_list", Description: "Remove a task list share", InputSchema: todoSchema(`{"id":{"type":"string"},"user_id":{"type":"string"}}`, "id", "user_id"), Effect: apps.AssistantExternal},
		{Name: "get_todo_preferences", Description: "Read Todo preferences", InputSchema: todoSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_todo_preferences", Description: "Update Todo preferences", InputSchema: todoSchema(`{"default_list_id":{"type":"string"},"sort_mode":{"type":"string"},"smart_collections":{"type":"object"}}`), Effect: apps.AssistantSafeWrite},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	var idInput struct {
		ID     string `json:"id"`
		UserID string `json:"user_id"`
	}
	_ = json.Unmarshal(arguments, &idInput)
	taskTarget := "/api/todo/tasks/" + url.PathEscape(idInput.ID)
	listTarget := "/api/todo/lists/" + url.PathEscape(idInput.ID)
	switch name {
	case "list_task_lists":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleLists, http.MethodGet, "/api/todo/lists", todoQueryArgs(arguments))
	case "read_task_list":
		items, err := a.lists(userID, false)
		if err != nil {
			return apps.AssistantResult{}, err
		}
		trashed, err := a.lists(userID, true)
		if err != nil {
			return apps.AssistantResult{}, err
		}
		items = append(items, trashed...)
		for _, item := range items {
			if item.ID == idInput.ID {
				data, _ := json.Marshal(item)
				return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(data)}}}, nil
			}
		}
		return apps.AssistantResult{}, fmt.Errorf("task list not found")
	case "list_tasks":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTasks, http.MethodGet, "/api/todo/tasks", todoQueryArgs(arguments))
	case "search_tasks":
		var search struct {
			Q string `json:"q"`
		}
		_ = json.Unmarshal(arguments, &search)
		search.Q = strings.ToLower(strings.TrimSpace(search.Q))
		items, err := a.tasks(userID, "", "", false)
		if err != nil {
			return apps.AssistantResult{}, err
		}
		filtered := make([]Task, 0)
		for _, item := range items {
			haystack := strings.ToLower(item.Title + " " + item.Notes + " " + strings.Join(item.Tags, " "))
			if strings.Contains(haystack, search.Q) {
				filtered = append(filtered, item)
				if len(filtered) == 200 {
					break
				}
			}
		}
		data, _ := json.Marshal(map[string]any{"tasks": filtered})
		return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(data)}}}, nil
	case "read_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTask, http.MethodGet, taskTarget, nil)
	case "create_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleLists, http.MethodPost, "/api/todo/lists", arguments)
	case "update_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodPatch, listTarget, withoutID(arguments))
	case "trash_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodDelete, listTarget, nil)
	case "restore_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodPost, listTarget+"/restore", nil)
	case "create_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTasks, http.MethodPost, "/api/todo/tasks", arguments)
	case "update_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTask, http.MethodPatch, taskTarget, withoutID(arguments))
	case "complete_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTask, http.MethodPost, taskTarget+"?action=complete", nil)
	case "trash_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTask, http.MethodDelete, taskTarget, nil)
	case "restore_task":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleTask, http.MethodPost, taskTarget+"?action=restore", nil)
	case "list_task_list_shares":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodGet, listTarget+"/share", nil)
	case "share_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodPost, listTarget+"/share", withoutID(arguments))
	case "unshare_task_list":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleList, http.MethodDelete, listTarget+"/share?user_id="+url.QueryEscape(idInput.UserID), nil)
	case "get_todo_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handlePreferences, http.MethodGet, "/api/todo/preferences", nil)
	case "set_todo_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handlePreferences, http.MethodPut, "/api/todo/preferences", arguments)
	case "delete_task_permanently":
		_, permission, err := a.getTask(userID, idInput.ID)
		if err != nil || permission == "" {
			return apps.AssistantResult{}, fmt.Errorf("task not found")
		}
		if err := a.exec(`DELETE FROM todo_tasks WHERE id=? AND deleted_at<>0`, idInput.ID); err != nil {
			return apps.AssistantResult{}, err
		}
		return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: `{"status":"ok"}`}}}, nil
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown todo tool %q", name)
	}
}

func todoQueryArgs(raw json.RawMessage) json.RawMessage {
	var value map[string]any
	if json.Unmarshal(raw, &value) == nil {
		if trash, ok := value["trash"].(bool); ok {
			if trash {
				value["trash"] = 1
			} else {
				delete(value, "trash")
			}
		}
	}
	data, _ := json.Marshal(value)
	return data
}

func withoutID(raw json.RawMessage) json.RawMessage {
	var value map[string]any
	if json.Unmarshal(raw, &value) == nil {
		delete(value, "id")
	}
	data, _ := json.Marshal(value)
	return data
}

func todoSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
