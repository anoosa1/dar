package news

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{
		{Name: "list_news_folders", Description: "List News folders", InputSchema: newsSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "list_news_feeds", Description: "List subscribed News feeds", InputSchema: newsSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "list_news_items", Description: "List News items with bounded filtering", InputSchema: newsSchema(`{"type":{"type":"integer"},"id":{"type":"integer"},"getRead":{"type":"boolean"},"batchSize":{"type":"integer","minimum":-1,"maximum":200},"offset":{"type":"integer"},"oldestFirst":{"type":"boolean"}}`), Effect: apps.AssistantRead},
		{Name: "create_news_folder", Description: "Create a News folder", InputSchema: newsSchema(`{"name":{"type":"string"}}`, "name"), Effect: apps.AssistantSafeWrite},
		{Name: "rename_news_folder", Description: "Rename a News folder", InputSchema: newsSchema(`{"id":{"type":"integer"},"name":{"type":"string"}}`, "id", "name"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_news_folder", Description: "Delete a News folder", InputSchema: newsSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "add_news_feed", Description: "Subscribe to a public RSS or Atom URL", InputSchema: newsSchema(`{"url":{"type":"string"},"folderId":{"type":"integer"},"autoDiscover":{"type":"boolean"}}`, "url"), Effect: apps.AssistantExternal},
		{Name: "move_news_feed", Description: "Move a News feed to a folder", InputSchema: newsSchema(`{"id":{"type":"integer"},"folderId":{"type":"integer"}}`, "id", "folderId"), Effect: apps.AssistantSafeWrite},
		{Name: "rename_news_feed", Description: "Rename a News feed", InputSchema: newsSchema(`{"id":{"type":"integer"},"feedTitle":{"type":"string"}}`, "id", "feedTitle"), Effect: apps.AssistantSafeWrite},
		{Name: "remove_news_feed", Description: "Remove a News subscription", InputSchema: newsSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "set_news_item_state", Description: "Mark News items read, unread, starred, or unstarred", InputSchema: newsSchema(`{"action":{"type":"string","enum":["read","unread","star","unstar"]},"itemIds":{"type":"array","items":{"type":"integer"},"maxItems":200}}`, "action", "itemIds"), Effect: apps.AssistantSafeWrite},
		{Name: "refresh_news", Description: "Fetch subscribed feeds now", InputSchema: newsSchema(`{}`), Effect: apps.AssistantExternal},
		{Name: "get_news_preferences", Description: "Read News preferences", InputSchema: newsSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_news_preferences", Description: "Update News preferences", InputSchema: newsSchema(`{"showAll":{"type":"boolean"},"sortOrder":{"type":"integer"},"compact":{"type":"boolean"}}`), Effect: apps.AssistantSafeWrite},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	var input map[string]any
	if err := json.Unmarshal(arguments, &input); err != nil {
		return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
	}
	id := fmt.Sprint(input["id"])
	base := v13Base
	switch name {
	case "list_news_folders":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodGet, base+"/folders", nil)
	case "list_news_feeds":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodGet, base+"/feeds", nil)
	case "list_news_items":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodGet, base+"/items", arguments)
	case "create_news_folder":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPost, base+"/folders", arguments)
	case "rename_news_folder":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPut, base+"/folders/"+url.PathEscape(id), withoutNewsID(arguments))
	case "delete_news_folder":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodDelete, base+"/folders/"+url.PathEscape(id), nil)
	case "add_news_feed":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPost, base+"/feeds", arguments)
	case "move_news_feed":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPost, base+"/feeds/"+url.PathEscape(id)+"/move", withoutNewsID(arguments))
	case "rename_news_feed":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPost, base+"/feeds/"+url.PathEscape(id)+"/rename", withoutNewsID(arguments))
	case "remove_news_feed":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodDelete, base+"/feeds/"+url.PathEscape(id), nil)
	case "set_news_item_state":
		action := fmt.Sprint(input["action"])
		delete(input, "action")
		body, _ := json.Marshal(input)
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleV13, http.MethodPost, base+"/items/"+url.PathEscape(action)+"/multiple", body)
	case "refresh_news":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAux, http.MethodPost, "/news/api/refresh", nil)
	case "get_news_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAux, http.MethodGet, "/news/api/preferences", nil)
	case "set_news_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAux, http.MethodPut, "/news/api/preferences", arguments)
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown news tool %q", name)
	}
}

func withoutNewsID(raw json.RawMessage) json.RawMessage {
	var value map[string]any
	_ = json.Unmarshal(raw, &value)
	delete(value, "id")
	data, _ := json.Marshal(value)
	return data
}
func newsSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
