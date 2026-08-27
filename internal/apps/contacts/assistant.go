package contacts

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) AssistantTools() []apps.AssistantTool {
	typedValue := `{"type":"object","properties":{"value":{"type":"string"},"types":{"type":"array","items":{"type":"string"}}},"required":["value"],"additionalProperties":false}`
	contactFields := `{"id":{"type":"string"},"book_id":{"type":"string"},"first_name":{"type":"string"},"last_name":{"type":"string"},"middle_name":{"type":"string"},"prefix":{"type":"string"},"suffix":{"type":"string"},"email":{"type":"string"},"phone":{"type":"string"},"emails":{"type":"array","items":` + typedValue + `},"phones":{"type":"array","items":` + typedValue + `},"org":{"type":"string"},"title":{"type":"string"},"role":{"type":"string"},"notes":{"type":"string"},"birthday":{"type":"string"},"groups":{"type":"array","items":{"type":"string"}},"favorite":{"type":"boolean"}}`
	return []apps.AssistantTool{
		{Name: "list_address_books", Description: "List the user's address books", InputSchema: contactsSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "list_contacts", Description: "List contacts, optionally in one address book", InputSchema: contactsSchema(`{"book":{"type":"string"}}`), Effect: apps.AssistantRead},
		{Name: "search_contacts", Description: "Search contacts", InputSchema: contactsSchema(`{"q":{"type":"string"}}`, "q"), Effect: apps.AssistantRead},
		{Name: "read_contact", Description: "Read one contact", InputSchema: contactsSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantRead},
		{Name: "create_contact", Description: "Create a person or organization contact", InputSchema: contactsSchema(contactFields), Effect: apps.AssistantSafeWrite},
		{Name: "update_contact", Description: "Update a contact", InputSchema: contactsSchema(contactFields, "id"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_contact", Description: "Permanently delete a contact", InputSchema: contactsSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
		{Name: "create_address_book", Description: "Create an address book", InputSchema: contactsSchema(`{"name":{"type":"string"}}`, "name"), Effect: apps.AssistantSafeWrite},
		{Name: "rename_address_book", Description: "Rename an address book", InputSchema: contactsSchema(`{"id":{"type":"string"},"name":{"type":"string"}}`, "id", "name"), Effect: apps.AssistantSafeWrite},
		{Name: "set_address_book_visibility", Description: "Show or hide an address book", InputSchema: contactsSchema(`{"id":{"type":"string"},"visible":{"type":"boolean"}}`, "id", "visible"), Effect: apps.AssistantSafeWrite},
		{Name: "delete_address_book", Description: "Permanently delete an address book and its contacts", InputSchema: contactsSchema(`{"id":{"type":"string"}}`, "id"), Effect: apps.AssistantDestructive},
	}
}

func (a *App) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	var handler http.HandlerFunc
	method, target := http.MethodGet, ""
	switch name {
	case "list_address_books":
		handler, target = a.handleBooks, "/api/contacts/books"
	case "list_contacts":
		handler, target = a.handleList, "/api/contacts/list"
	case "search_contacts":
		handler, target = a.handleSearch, "/api/contacts/search"
	case "read_contact":
		handler, target = a.handleGet, "/api/contacts/get"
	case "create_contact":
		handler, method, target = a.handleCreate, http.MethodPost, "/api/contacts/create"
	case "update_contact":
		handler, method, target = a.handleUpdate, http.MethodPost, "/api/contacts/update"
	case "create_address_book":
		handler, method, target = a.handleBooks, http.MethodPost, "/api/contacts/books"
	case "rename_address_book":
		handler, method, target = a.handleBooksRename, http.MethodPost, "/api/contacts/books/rename"
	case "set_address_book_visibility":
		handler, method, target = a.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility"
	case "delete_address_book":
		handler, method, target = a.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete"
	case "delete_contact":
		var input struct {
			ID string `json:"id"`
		}
		if err := json.Unmarshal(arguments, &input); err != nil {
			return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
		}
		target = "/api/contacts/delete?" + url.Values{"id": {input.ID}}.Encode()
		return apps.InvokeAssistantHTTP(ctx, userID, a.handleDelete, http.MethodPost, target, nil)
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown contacts tool %q", name)
	}
	return apps.InvokeAssistantHTTP(ctx, userID, handler, method, target, arguments)
}

func contactsSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
