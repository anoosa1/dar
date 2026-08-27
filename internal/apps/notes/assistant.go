package notes

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"path"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{
		{Name: "list_notes", Description: "List or search notes, optionally by category", InputSchema: notesSchema(`{"q":{"type":"string"},"category":{"type":"string"},"summary":{"type":"integer","enum":[0,1]}}`), Effect: apps.AssistantRead},
		{Name: "read_note", Description: "Read a note", InputSchema: notesSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "create_note", Description: "Create a Markdown note", InputSchema: notesSchema(`{"title":{"type":"string"},"content":{"type":"string"},"category":{"type":"string"},"favorite":{"type":"boolean"}}`), Effect: apps.AssistantSafeWrite},
		{Name: "update_note", Description: "Update a Markdown note", InputSchema: notesSchema(`{"id":{"type":"integer"},"title":{"type":"string"},"content":{"type":"string"},"category":{"type":"string"},"favorite":{"type":"boolean"},"force":{"type":"boolean"}}`, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_note", Description: "Delete a note", InputSchema: notesSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "list_note_categories", Description: "List note categories", InputSchema: notesSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "create_note_category", Description: "Create a note category", InputSchema: notesSchema(`{"name":{"type":"string"}}`, "name"), Effect: apps.AssistantSafeWrite},
		{Name: "rename_note_category", Description: "Rename a note category", InputSchema: notesSchema(`{"from":{"type":"string"},"to":{"type":"string"}}`, "from", "to"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_note_category", Description: "Delete a note category", InputSchema: notesSchema(`{"name":{"type":"string"},"force":{"type":"boolean"}}`, "name"), Effect: apps.AssistantDestructive},
		{Name: "list_note_versions", Description: "List stored versions of a note", InputSchema: notesSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "restore_note_version", Description: "Restore a prior note version", InputSchema: notesSchema(`{"id":{"type":"integer"},"version":{"type":"string"}}`, "id", "version"), Effect: apps.AssistantDestructive},
		{Name: "undo_note", Description: "Undo the last note change", InputSchema: notesSchema(`{"id":{"type":"integer"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "read_note_attachment", Description: "Read an image or text attachment belonging to a note", InputSchema: notesSchema(`{"id":{"type":"integer"},"path":{"type":"string"}}`, "id", "path"), Effect: apps.AssistantRead},
		{Name: "upload_note_attachment", Description: "Attach a base64-encoded image to a note", InputSchema: notesSchema(`{"id":{"type":"integer"},"filename":{"type":"string"},"data_base64":{"type":"string"}}`, "id", "filename", "data_base64"), Effect: apps.AssistantSafeWrite},
		{Name: "get_notes_preferences", Description: "Read Notes preferences", InputSchema: notesSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_notes_preferences", Description: "Update Notes preferences", InputSchema: notesSchema(`{"notesPath":{"type":"string"},"fileSuffix":{"type":"string"},"editorMode":{"type":"string"}}`), Effect: apps.AssistantSafeWrite},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	var input map[string]any
	if err := json.Unmarshal(arguments, &input); err != nil {
		return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
	}
	id := fmt.Sprint(input["id"])
	params := map[string]string{"id": id}
	switch name {
	case "list_notes":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleNotesCollection, http.MethodGet, "/api/notes/notes", arguments)
	case "read_note":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleNote, http.MethodGet, "/api/notes/notes/"+id, nil, params)
	case "create_note":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleNotesCollection, http.MethodPost, "/api/notes/notes", arguments)
	case "update_note":
		force := ""
		if value, _ := input["force"].(bool); value {
			force = "?force=1"
		}
		delete(input, "id")
		delete(input, "force")
		body, _ := json.Marshal(input)
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleNote, http.MethodPut, "/api/notes/notes/"+id+force, body, params)
	case "delete_note":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleNote, http.MethodDelete, "/api/notes/notes/"+id, nil, params)
	case "list_note_categories":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCategories, http.MethodGet, "/api/notes/categories", nil)
	case "create_note_category":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCategories, http.MethodPost, "/api/notes/categories", arguments)
	case "rename_note_category":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCategories, http.MethodPut, "/api/notes/categories", arguments)
	case "delete_note_category":
		q := url.Values{"name": {fmt.Sprint(input["name"])}}
		if force, _ := input["force"].(bool); force {
			q.Set("force", "1")
		}
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleCategories, http.MethodDelete, "/api/notes/categories?"+q.Encode(), nil)
	case "list_note_versions":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleVersions, http.MethodGet, "/api/notes/versions/"+id, nil, params)
	case "restore_note_version":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleVersions, http.MethodPost, "/api/notes/versions/"+id+"?version="+url.QueryEscape(fmt.Sprint(input["version"])), nil, params)
	case "undo_note":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleUndo, http.MethodPost, "/api/notes/undo/"+id, nil, params)
	case "read_note_attachment":
		return apps.InvokeAssistantHTTPParams(ctx, userID, a.handleAttachment, http.MethodGet, "/api/notes/attachment/"+id+"?path="+url.QueryEscape(fmt.Sprint(input["path"])), nil, params)
	case "upload_note_attachment":
		encoded, _ := input["data_base64"].(string)
		data, err := base64.StdEncoding.DecodeString(encoded)
		if err != nil || len(data) == 0 || len(data) > 10<<20 {
			return apps.AssistantResult{}, fmt.Errorf("image data must be valid base64 up to 10 MiB")
		}
		_, ext, ok := validImageType(data, fmt.Sprint(input["filename"]))
		if !ok {
			return apps.AssistantResult{}, fmt.Errorf("unsupported image")
		}
		noteID, err := routeIDFromString(id)
		if err != nil {
			return apps.AssistantResult{}, err
		}
		note, err := a.note(ctx, userID, noteID)
		if err != nil {
			return apps.AssistantResult{}, err
		}
		filename := randomName(ext)
		if err := a.files.WriteFile(ctx, userID, path.Join(path.Dir(note.Path), filename), data); err != nil {
			return apps.AssistantResult{}, err
		}
		result, _ := json.Marshal(map[string]string{"filename": filename})
		return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(result)}}}, nil
	case "get_notes_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSettings, http.MethodGet, "/api/notes/settings", nil)
	case "set_notes_preferences":
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleSettings, http.MethodPut, "/api/notes/settings", arguments)
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown notes tool %q", name)
	}
}

func routeIDFromString(raw string) (int64, error) { return parseID(raw) }

func notesSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
