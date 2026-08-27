package files

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"path"

	"github.com/anoosa/dar/internal/apps"
)

var fileAssistantTools = []apps.AssistantTool{
	{Name: "list", Description: "List files and folders at a user-rooted Dar path.", InputSchema: schema(`{"path":{"type":"string"}}`), Effect: apps.AssistantRead},
	{Name: "search", Description: "Search the user's Dar files by name.", InputSchema: schema(`{"q":{"type":"string"}}`), Effect: apps.AssistantRead},
	{Name: "read", Description: "Read or download a file at a user-rooted Dar path. Large results are truncated.", InputSchema: requiredSchema(`{"path":{"type":"string"}}`, "path"), Effect: apps.AssistantRead},
	{Name: "write_text", Description: "Create or replace a UTF-8 text file.", InputSchema: requiredSchema(`{"path":{"type":"string"},"content":{"type":"string"}}`, "path", "content"), Effect: apps.AssistantSafeWrite},
	{Name: "create_folder", Description: "Create a folder and any missing parents.", InputSchema: requiredSchema(`{"path":{"type":"string"}}`, "path"), Effect: apps.AssistantSafeWrite},
	{Name: "create_symlink", Description: "Create a root-confined symbolic link. The target may be user-rooted absolute or relative to the link.", InputSchema: requiredSchema(`{"path":{"type":"string"},"target":{"type":"string"}}`, "path", "target"), Effect: apps.AssistantSafeWrite},
	{Name: "rename", Description: "Rename a file or folder.", InputSchema: requiredSchema(`{"path":{"type":"string"},"name":{"type":"string"}}`, "path", "name"), Effect: apps.AssistantSafeWrite},
	{Name: "move", Description: "Move a file or folder to another Dar path.", InputSchema: requiredSchema(`{"from":{"type":"string"},"to":{"type":"string"}}`, "from", "to"), Effect: apps.AssistantSafeWrite},
	{Name: "copy", Description: "Copy a file or folder to another Dar path.", InputSchema: requiredSchema(`{"from":{"type":"string"},"to":{"type":"string"}}`, "from", "to"), Effect: apps.AssistantSafeWrite},
	{Name: "favorite", Description: "Set or clear a file's favorite state.", InputSchema: requiredSchema(`{"path":{"type":"string"},"favorite":{"type":"boolean"}}`, "path", "favorite"), Effect: apps.AssistantSafeWrite},
	{Name: "tag", Description: "Add or remove a Dar file tag.", InputSchema: requiredSchema(`{"path":{"type":"string"},"tag_id":{"type":"string"},"action":{"type":"string","enum":["add","remove"]}}`, "path", "tag_id", "action"), Effect: apps.AssistantSafeWrite},
	{Name: "share", Description: "Create or change a share for a file or folder.", InputSchema: schema(`{"path":{"type":"string"},"action":{"type":"string"},"to_user":{"type":"string"},"permission":{"type":"string"}}`), Effect: apps.AssistantExternal},
	{Name: "trash", Description: "Move a file or folder to Dar trash.", InputSchema: requiredSchema(`{"path":{"type":"string"}}`, "path"), Effect: apps.AssistantDestructive},
	{Name: "restore", Description: "Restore a trashed file by trash id.", InputSchema: requiredSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantSafeWrite},
	{Name: "delete_permanently", Description: "Permanently remove a trashed file.", InputSchema: requiredSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
}

func (a *App) AssistantTools() []apps.AssistantTool {
	return append([]apps.AssistantTool(nil), fileAssistantTools...)
}

func (a *App) CallAssistantTool(ctx context.Context, uid, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	switch name {
	case "list":
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleList, http.MethodGet, "/api/files/list", arguments)
	case "search":
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleSearch, http.MethodGet, "/api/files/search", arguments)
	case "read":
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleDownload, http.MethodGet, "/api/files/download", arguments)
	case "create_folder":
		var input struct{ Path string }
		if json.Unmarshal(arguments, &input) != nil || input.Path == "" {
			return apps.AssistantResult{}, fmt.Errorf("path is required")
		}
		if err := a.MkdirAll(ctx, uid, input.Path); err != nil {
			return apps.AssistantResult{}, err
		}
		return textResult(`{"status":"ok"}`), nil
	case "create_symlink":
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleSymlink, http.MethodPost, "/api/files/symlink", arguments)
	case "rename":
		var input struct{ Path, Name string }
		if json.Unmarshal(arguments, &input) != nil || input.Path == "" || !editorName(input.Name) {
			return apps.AssistantResult{}, fmt.Errorf("path and valid name are required")
		}
		destination := path.Join(path.Dir(input.Path), input.Name)
		query := url.Values{"src": {input.Path}, "dst": {destination}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleRename, http.MethodPost, "/api/files/rename?"+query.Encode(), nil)
	case "move":
		var input struct{ From, To string }
		if json.Unmarshal(arguments, &input) != nil || input.From == "" || input.To == "" {
			return apps.AssistantResult{}, fmt.Errorf("from and to are required")
		}
		query := url.Values{"src": {input.From}, "dst": {input.To}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleMove, http.MethodPost, "/api/files/move?"+query.Encode(), nil)
	case "copy":
		var input struct{ From, To string }
		if json.Unmarshal(arguments, &input) != nil || input.From == "" || input.To == "" {
			return apps.AssistantResult{}, fmt.Errorf("from and to are required")
		}
		query := url.Values{"src": {input.From}, "dst": {input.To}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleCopy, http.MethodPost, "/api/files/copy?"+query.Encode(), nil)
	case "favorite":
		var input struct {
			Path     string
			Favorite bool
		}
		if json.Unmarshal(arguments, &input) != nil || input.Path == "" {
			return apps.AssistantResult{}, fmt.Errorf("path is required")
		}
		input.Path = a.cleanVPath(input.Path)
		if a.loadFavorites(uid)[input.Path] == input.Favorite {
			return textResult(`{"status":"ok"}`), nil
		}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleFavorite, http.MethodPost, "/api/files/favorite?"+url.Values{"path": {input.Path}}.Encode(), nil)
	case "tag":
		var input struct {
			Path   string `json:"path"`
			TagID  string `json:"tag_id"`
			Action string `json:"action"`
		}
		if json.Unmarshal(arguments, &input) != nil || input.Path == "" || input.TagID == "" || (input.Action != "add" && input.Action != "remove") {
			return apps.AssistantResult{}, fmt.Errorf("path, tag_id, and action are required")
		}
		body, _ := json.Marshal(map[string]any{"path": input.Path, "tag": input.TagID, "add": input.Action == "add"})
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleTag, http.MethodPost, "/api/files/tag", body)
	case "share":
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleShare, http.MethodPost, "/api/files/share", arguments)
	case "trash":
		var input struct{ Path string }
		if json.Unmarshal(arguments, &input) != nil || input.Path == "" {
			return apps.AssistantResult{}, fmt.Errorf("path is required")
		}
		query := url.Values{"path": {input.Path}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleDelete, http.MethodPost, "/api/files/delete?"+query.Encode(), nil)
	case "restore":
		var input struct{ ID string }
		if json.Unmarshal(arguments, &input) != nil || input.ID == "" {
			return apps.AssistantResult{}, fmt.Errorf("id is required")
		}
		query := url.Values{"id": {input.ID}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleRestore, http.MethodPost, "/api/files/restore?"+query.Encode(), nil)
	case "delete_permanently":
		var input struct{ ID string }
		if json.Unmarshal(arguments, &input) != nil || input.ID == "" {
			return apps.AssistantResult{}, fmt.Errorf("id is required")
		}
		query := url.Values{"id": {input.ID}}
		return apps.InvokeAssistantHTTP(ctx, uid, a.handleDeletePermanent, http.MethodPost, "/api/files/delete-permanent?"+query.Encode(), nil)
	case "write_text":
		var in struct{ Path, Content string }
		if err := json.Unmarshal(arguments, &in); err != nil || in.Path == "" {
			return apps.AssistantResult{}, fmt.Errorf("path and content are required")
		}
		if err := a.WriteFile(ctx, uid, in.Path, []byte(in.Content)); err != nil {
			return apps.AssistantResult{}, err
		}
		return textResult(`{"ok":true}`), nil
	}
	return apps.AssistantResult{}, fmt.Errorf("unknown files tool %q", name)
}

func schema(properties string) json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":` + properties + `,"additionalProperties":false}`)
}
func requiredSchema(properties string, required ...string) json.RawMessage {
	data, _ := json.Marshal(required)
	return json.RawMessage(`{"type":"object","properties":` + properties + `,"required":` + string(data) + `,"additionalProperties":false}`)
}
func textResult(text string) apps.AssistantResult {
	return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: text}}}
}
