package contacts

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/anoosa/dar/internal/apps"
)

func contactAssistantText(t *testing.T, app *App, ctx context.Context, uid, name, args string) string {
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

func TestContactsAssistantFullWorkflow(t *testing.T) {
	withTempDataRoot(t)
	ctx := context.Background()
	uid := "assistant-user"
	bookText := contactAssistantText(t, testApp, ctx, uid, "create_address_book", `{"name":"Work contacts"}`)
	var book AddressBook
	if err := json.Unmarshal([]byte(bookText), &book); err != nil {
		t.Fatal(err)
	}
	contactText := contactAssistantText(t, testApp, ctx, uid, "create_contact", `{"book_id":"`+book.ID+`","first_name":"Ada","last_name":"Lovelace","email":"ada@example.test","phone":"+1 555","org":"Engines","notes":"search target"}`)
	var contact Contact
	if err := json.Unmarshal([]byte(contactText), &contact); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name string
		args string
	}{
		{"list_address_books", `{}`},
		{"list_contacts", `{"book":"` + book.ID + `"}`},
		{"search_contacts", `{"q":"search"}`},
		{"read_contact", `{"id":"` + contact.ID + `"}`},
	} {
		contactAssistantText(t, testApp, ctx, uid, tc.name, tc.args)
	}
	contactAssistantText(t, testApp, ctx, uid, "update_contact", `{"id":"`+contact.ID+`","book_id":"`+book.ID+`","first_name":"Augusta","favorite":true}`)
	contactAssistantText(t, testApp, ctx, uid, "rename_address_book", `{"id":"`+book.ID+`","name":"Archive"}`)
	contactAssistantText(t, testApp, ctx, uid, "set_address_book_visibility", `{"id":"`+book.ID+`","visible":false}`)
	contactAssistantText(t, testApp, ctx, uid, "delete_contact", `{"id":"`+contact.ID+`"}`)
	contactAssistantText(t, testApp, ctx, uid, "delete_address_book", `{"id":"`+book.ID+`"}`)
	for _, tc := range []struct {
		name string
		args string
	}{
		{"delete_contact", `{`},
		{"unknown", `{}`},
	} {
		if _, err := testApp.CallAssistantTool(ctx, uid, tc.name, json.RawMessage(tc.args)); err == nil {
			t.Errorf("%s unexpectedly succeeded", tc.name)
		}
	}
	if len(testApp.AssistantTools()) == 0 || apps.AssistantRead == "" {
		t.Fatal("assistant tool manifest missing")
	}
}
