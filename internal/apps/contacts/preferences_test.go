package contacts

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"

	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

func preferencesTestApp(t *testing.T) *App {
	t.Helper()
	database, err := db.Open(filepath.Join(t.TempDir(), "contacts.sqlite"), preferences.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{DB: database, DataRoot: t.TempDir()})
}

func preferencesRequest(t *testing.T, app *App, method, userID, body string) *httptest.ResponseRecorder {
	t.Helper()
	request := httptest.NewRequest(method, "/api/contacts/preferences", bytes.NewBufferString(body))
	request = withTestUser(request, userID)
	response := httptest.NewRecorder()
	app.handlePreferences(response, request)
	return response
}

func TestContactsPreferencesDefaultValidationAndPersistence(t *testing.T) {
	app := preferencesTestApp(t)
	response := preferencesRequest(t, app, http.MethodGet, "alice", "")
	if response.Code != http.StatusOK {
		t.Fatalf("default status=%d body=%s", response.Code, response.Body.String())
	}
	if got := decodeBody[ContactsPreferences](t, response); got.SortBy != "display_name" {
		t.Fatalf("default preferences = %#v", got)
	}

	response = preferencesRequest(t, app, http.MethodPut, "alice", `{"sort_by":"last_name"}`)
	if response.Code != http.StatusOK {
		t.Fatalf("save status=%d body=%s", response.Code, response.Body.String())
	}
	response = preferencesRequest(t, app, http.MethodGet, "alice", "")
	if got := decodeBody[ContactsPreferences](t, response); got.SortBy != "last_name" {
		t.Fatalf("persisted preferences = %#v", got)
	}
	response = preferencesRequest(t, app, http.MethodGet, "bob", "")
	if got := decodeBody[ContactsPreferences](t, response); got.SortBy != "display_name" {
		t.Fatalf("cross-user preferences = %#v", got)
	}

	response = preferencesRequest(t, app, http.MethodPut, "alice", `{"sort_by":"nickname"}`)
	if response.Code != http.StatusBadRequest {
		t.Fatalf("invalid status=%d body=%s", response.Code, response.Body.String())
	}
}

func TestContactsPreferencesJSONShape(t *testing.T) {
	data, err := json.Marshal(defaultContactsPreferences())
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != `{"sort_by":"display_name"}` {
		t.Fatalf("JSON = %s", data)
	}
}

func TestContactPreferenceSortKeys(t *testing.T) {
	ada := Contact{FirstName: "Ada", LastName: "Zulu"}
	grace := Contact{FirstName: "Grace", LastName: "Alpha"}
	if !(ada.preferenceSortKey("first_name") < grace.preferenceSortKey("first_name")) {
		t.Fatal("first-name sorting did not order Ada before Grace")
	}
	if !(grace.preferenceSortKey("last_name") < ada.preferenceSortKey("last_name")) {
		t.Fatal("last-name sorting did not order Alpha before Zulu")
	}
}
