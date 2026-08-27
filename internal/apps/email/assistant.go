package email

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	draft := `{"id":{"type":"integer"},"account_id":{"type":"string"},"to":{"type":"string"},"cc":{"type":"string"},"bcc":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"},"in_reply_to":{"type":"string"},"references":{"type":"string"}}`
	action := `{"message_ids":{"type":"array","items":{"type":"integer"},"maxItems":200}}`
	return []apps.AssistantTool{
		{Name: "list_email_accounts", Description: "List email account metadata without credentials", InputSchema: emailSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "list_mailboxes", Description: "List mailboxes and unread counts", InputSchema: emailSchema(`{"account_id":{"type":"string"}}`), Effect: apps.AssistantRead},
		{Name: "search_email_threads", Description: "Search or list email threads", InputSchema: emailSchema(`{"account_id":{"type":"string"},"role":{"type":"string"},"folder_id":{"type":"integer"},"q":{"type":"string"},"unread":{"type":"boolean"},"flagged":{"type":"boolean"},"has_attachment":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string"}}`), Effect: apps.AssistantRead},
		{Name: "read_email_thread", Description: "Read messages in an email thread", InputSchema: emailSchema(`{"thread_id":{"type":"string"}}`, "thread_id"), Effect: apps.AssistantRead},
		{Name: "read_email_message", Description: "Read one email message with attachment metadata", InputSchema: emailSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "create_email_draft", Description: "Create an email draft", InputSchema: emailSchema(draft, "account_id"), Effect: apps.AssistantSafeWrite},
		{Name: "read_email_draft", Description: "Read an email draft", InputSchema: emailSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "update_email_draft", Description: "Update an email draft", InputSchema: emailSchema(draft, "id", "account_id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_email_draft", Description: "Permanently delete an email draft", InputSchema: emailSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "send_email_draft", Description: "Send an email draft", InputSchema: emailSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantExternal},
		{Name: "move_email_messages", Description: "Move email messages to another folder", InputSchema: emailSchema(`{"message_ids":{"type":"array","items":{"type":"integer"},"maxItems":200},"destination_folder":{"type":"integer"}}`, "message_ids", "destination_folder"), Effect: apps.AssistantSafeWrite},
		{Name: "mark_email_read", Description: "Mark email messages read", InputSchema: emailSchema(action, "message_ids"), Effect: apps.AssistantSafeWrite},
		{Name: "mark_email_unread", Description: "Mark email messages unread", InputSchema: emailSchema(action, "message_ids"), Effect: apps.AssistantSafeWrite},
		{Name: "flag_email_messages", Description: "Flag email messages", InputSchema: emailSchema(action, "message_ids"), Effect: apps.AssistantSafeWrite},
		{Name: "unflag_email_messages", Description: "Remove flags from email messages", InputSchema: emailSchema(action, "message_ids"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_email_messages", Description: "Delete email messages", InputSchema: emailSchema(action, "message_ids"), Effect: apps.AssistantDestructive},
		{Name: "get_email_preferences", Description: "Read Email preferences", InputSchema: emailSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_email_preferences", Description: "Update Email preferences", InputSchema: emailSchema(`{"default_account":{"type":"string"},"density":{"type":"string"},"reading_pane":{"type":"string"}}`), Effect: apps.AssistantSafeWrite},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	var input map[string]any
	if err := json.Unmarshal(arguments, &input); err != nil {
		return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
	}
	id := fmt.Sprint(input["id"])
	switch name {
	case "list_email_accounts":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/accounts", nil)
	case "list_mailboxes":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/mailboxes", arguments)
	case "search_email_threads":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/threads", arguments)
	case "read_email_thread":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/threads/"+url.PathEscape(fmt.Sprint(input["thread_id"])), nil)
	case "read_email_message":
		messageID, ok := input["id"].(float64)
		if !ok {
			return apps.AssistantResult{}, fmt.Errorf("id is required")
		}
		message, err := a.withMessage(ctx, userID, int64(messageID))
		if err != nil {
			return apps.AssistantResult{}, err
		}
		data, _ := json.Marshal(message)
		return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(data)}}}, nil
	case "create_email_draft":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodPost, "/api/email/drafts", arguments)
	case "read_email_draft":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/drafts/"+url.PathEscape(id), nil)
	case "update_email_draft":
		delete(input, "id")
		body, _ := json.Marshal(input)
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodPatch, "/api/email/drafts/"+url.PathEscape(id), body)
	case "delete_email_draft":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodDelete, "/api/email/drafts/"+url.PathEscape(id), nil)
	case "send_email_draft":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodPost, "/api/email/drafts/"+url.PathEscape(id)+"/send", nil)
	case "get_email_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodGet, "/api/email/preferences", nil)
	case "set_email_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodPut, "/api/email/preferences", arguments)
	}
	actions := map[string]string{"move_email_messages": "move", "mark_email_read": "read", "mark_email_unread": "unread", "flag_email_messages": "flag", "unflag_email_messages": "unflag", "delete_email_messages": "delete"}
	if action := actions[name]; action != "" {
		input["action"] = action
		body, _ := json.Marshal(input)
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleAPI, http.MethodPost, "/api/email/messages/actions", body)
	}
	return apps.AssistantResult{}, fmt.Errorf("unknown email tool %q", name)
}

func emailSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
