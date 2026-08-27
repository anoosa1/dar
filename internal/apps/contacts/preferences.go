package contacts

import (
	"encoding/json"
	"net/http"

	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
)

const contactsPreferencesKey = "settings"

type ContactsPreferences struct {
	SortBy string `json:"sort_by"`
}

func defaultContactsPreferences() ContactsPreferences {
	return ContactsPreferences{SortBy: "display_name"}
}

func validContactsSort(value string) bool {
	return value == "display_name" || value == "last_name" || value == "first_name"
}

func (app *App) loadPreferences(r *http.Request) (ContactsPreferences, error) {
	result := defaultContactsPreferences()
	if app.db == nil {
		return result, nil
	}
	user := server.GetUser(r)
	if user == nil {
		return result, nil
	}
	conn, release, err := app.db.Conn(r.Context())
	if err != nil {
		return result, err
	}
	defer release()
	if _, err := preferences.GetJSON(conn, user.ID, "contacts", contactsPreferencesKey, &result); err != nil {
		return result, err
	}
	if !validContactsSort(result.SortBy) {
		result.SortBy = "display_name"
	}
	return result, nil
}

func (app *App) handlePreferences(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		value, err := app.loadPreferences(r)
		if err != nil {
			http.Error(w, "could not load preferences", http.StatusInternalServerError)
			return
		}
		app.writeJSON(w, value)
	case http.MethodPut:
		var value ContactsPreferences
		decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&value); err != nil || !validContactsSort(value.SortBy) {
			http.Error(w, "invalid sort_by", http.StatusBadRequest)
			return
		}
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		if app.db == nil {
			http.Error(w, "preferences unavailable", http.StatusInternalServerError)
			return
		}
		conn, release, err := app.db.Conn(r.Context())
		if err != nil {
			http.Error(w, "could not save preferences", http.StatusInternalServerError)
			return
		}
		defer release()
		if err := preferences.SetJSON(conn, user.ID, "contacts", contactsPreferencesKey, value); err != nil {
			http.Error(w, "could not save preferences", http.StatusInternalServerError)
			return
		}
		app.writeJSON(w, value)
	default:
		w.Header().Set("Allow", "GET, PUT")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}
