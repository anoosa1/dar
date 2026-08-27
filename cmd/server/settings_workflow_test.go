package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
)

type meContactStub struct {
	name    string
	err     error
	calls   int
	userID  string
	profile auth.UserProfile
}

func (s *meContactStub) UpsertMe(_ context.Context, userID, displayName, _ string, profile auth.UserProfile) (string, error) {
	s.calls++
	s.userID = userID
	s.profile = profile
	if s.name == "" {
		s.name = displayName + " (Me)"
	}
	return s.name, s.err
}

func createSettingsUser(t *testing.T, srv *server.Server) *auth.User {
	t.Helper()
	conn, release, err := srv.DB.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	user, _, err := auth.FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://issuer.example", "alice-subject")
	if err != nil {
		t.Fatal(err)
	}
	return user
}

func settingsRequest(method, target string, body any, user *auth.User) (*httptest.ResponseRecorder, *http.Request) {
	var raw []byte
	switch value := body.(type) {
	case nil:
	case []byte:
		raw = value
	default:
		raw, _ = json.Marshal(value)
	}
	req := httptest.NewRequest(method, target, bytes.NewReader(raw))
	req.Header.Set("Content-Type", "application/json")
	if user != nil {
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
	}
	return httptest.NewRecorder(), req
}

func TestSettingsPagesAndHelpers(t *testing.T) {
	srv := testSettingsServer(t)
	registry := apps.NewRegistry()
	user := createSettingsUser(t, srv)

	for _, tc := range []struct {
		path    string
		section string
		needle  string
	}{
		{"/settings", "personal", "Personal info"},
		{"/settings/personal/", "personal", "Personal info"},
		{"/settings/security", "security", "App passwords"},
		{"/settings/appearance", "appearance", "Appearance"},
		{"/settings/unknown", "personal", "Personal info"},
	} {
		if got := settingsSection(tc.path); got != tc.section {
			t.Fatalf("settingsSection(%q)=%q", tc.path, got)
		}
		rec, req := settingsRequest(http.MethodGet, tc.path, nil, user)
		handleSettings(srv, registry)(rec, req)
		if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), tc.needle) {
			t.Fatalf("%s status=%d body=%s", tc.path, rec.Code, rec.Body.String())
		}
	}

	rec, req := settingsRequest(http.MethodGet, "/settings", nil, nil)
	handleSettings(srv, registry)(rec, req)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" {
		t.Fatalf("anonymous status=%d location=%q", rec.Code, rec.Header().Get("Location"))
	}

	active := settingsNavItem("/settings", "icon", "<Personal>", true)
	inactive := settingsNavItem("/settings", "icon", "Personal", false)
	if !strings.Contains(active, "bg-primary-container") || !strings.Contains(inactive, "hover:bg-surface") {
		t.Fatalf("navigation classes active=%q inactive=%q", active, inactive)
	}
	if got := profileInitial(""); got != "U" {
		t.Fatalf("empty initial=%q", got)
	}
	if got := profileInitial("<Alice"); got != "&lt;" {
		t.Fatalf("escaped initial=%q", got)
	}
	for _, section := range []string{"personal", "security", "appearance"} {
		if html := string(settingsPageHTML(user, section)); !strings.Contains(html, `id="settings-app"`) {
			t.Fatalf("%s page missing shell", section)
		}
	}

	rec, req = settingsRequest(http.MethodGet, "/search", nil, user)
	handleSearchPage(srv, registry)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Search") {
		t.Fatalf("search page status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodGet, "/search", nil, nil)
	handleSearchPage(srv, registry)(rec, req)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" {
		t.Fatalf("anonymous search status=%d location=%q", rec.Code, rec.Header().Get("Location"))
	}
}

func TestTimeFormatSettingsLifecycleAndValidation(t *testing.T) {
	srv := testSettingsServer(t)
	user := createSettingsUser(t, srv)

	rec, req := settingsRequest(http.MethodGet, "/api/settings/time-format", nil, user)
	handleGetTimeFormat(srv)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"time_format_24h":false`) {
		t.Fatalf("default status=%d body=%s", rec.Code, rec.Body.String())
	}

	rec, req = settingsRequest(http.MethodPost, "/api/settings/time-format", timeFormatSettings{TimeFormat24h: true}, user)
	handleSaveTimeFormat(srv)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"time_format_24h":true`) {
		t.Fatalf("save status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/time-format", nil, user)
	handleGetTimeFormat(srv)(rec, req)
	if !strings.Contains(rec.Body.String(), `"time_format_24h":true`) {
		t.Fatalf("persisted body=%s", rec.Body.String())
	}

	rec, req = settingsRequest(http.MethodPost, "/api/settings/time-format", []byte("{"), user)
	handleSaveTimeFormat(srv)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("malformed status=%d", rec.Code)
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/time-format", nil, nil)
	handleGetTimeFormat(srv)(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous get status=%d", rec.Code)
	}
	rec, req = settingsRequest(http.MethodPost, "/api/settings/time-format", timeFormatSettings{}, nil)
	handleSaveTimeFormat(srv)(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous save status=%d", rec.Code)
	}
}

func TestPersonalProfilePersistenceWeatherResetAndContactSync(t *testing.T) {
	srv := testSettingsServer(t)
	user := createSettingsUser(t, srv)
	user.Profile.Location = "Old City"
	conn, release, err := srv.DB.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if err := auth.UpdateUserProfile(conn, user.ID, user.DisplayName, user.Profile); err != nil {
		release()
		t.Fatal(err)
	}
	weather := weatherLocationPreference{
		Unit: "metric", LocationSource: "profile", LocationLabel: "Old City",
		Latitude: 43.65, Longitude: -79.38, Timezone: "America/Toronto",
	}
	if err := preferences.SetJSON(conn, user.ID, "weather", "settings", weather); err != nil {
		release()
		t.Fatal(err)
	}
	release()

	contacts := &meContactStub{name: "Ada (Me)"}
	body := personalSaveBody{
		DisplayName: "  Ada Lovelace  ", Phone: " +1 555 ", Address: " London ",
		Website: " https://example.test ", Twitter: " @ada ", Fediverse: " @ada@example.social ",
		Organisation: " Analytical Engines ", Role: " Programmer ", Headline: " Pioneer ",
		Biography: " Biography ", Location: " London ", Language: " en ", Locale: " en_GB ", Birthday: "1815-12-10",
	}
	rec, req := settingsRequest(http.MethodPost, "/api/settings/personal", body, user)
	handleSavePersonal(srv, contacts)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"contact_name":"Ada (Me)"`) {
		t.Fatalf("save status=%d body=%s", rec.Code, rec.Body.String())
	}
	if contacts.calls != 1 || contacts.userID != user.ID || contacts.profile.Location != "London" {
		t.Fatalf("contact sync=%+v", contacts)
	}

	conn, release, err = srv.DB.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	stored, err := auth.GetUserByID(conn, user.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.DisplayName != "Ada Lovelace" || stored.Profile.Phone != "+1 555" || stored.Profile.Location != "London" {
		t.Fatalf("stored user=%+v", stored)
	}
	var reset weatherLocationPreference
	ok, err := preferences.GetJSON(conn, user.ID, "weather", "settings", &reset)
	if err != nil || !ok {
		t.Fatalf("weather read ok=%v err=%v", ok, err)
	}
	if reset.Unit != "metric" || reset.LocationSource != "" || reset.LocationLabel != "" || reset.Latitude != 0 || reset.Longitude != 0 || reset.Timezone != "" {
		t.Fatalf("weather was not reset: %+v", reset)
	}
}

func TestPersonalProfileValidationAndContactWarning(t *testing.T) {
	srv := testSettingsServer(t)
	user := createSettingsUser(t, srv)
	contacts := &meContactStub{err: errors.New("contacts unavailable")}

	rec, req := settingsRequest(http.MethodPost, "/api/settings/personal", personalSaveBody{DisplayName: "Alice"}, user)
	handleSavePersonal(srv, contacts)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"warning"`) || !strings.Contains(rec.Body.String(), "contacts unavailable") {
		t.Fatalf("warning status=%d body=%s", rec.Code, rec.Body.String())
	}

	for _, tc := range []struct {
		method string
		body   any
		user   *auth.User
		status int
	}{
		{http.MethodGet, nil, user, http.StatusMethodNotAllowed},
		{http.MethodPost, []byte("{"), user, http.StatusBadRequest},
		{http.MethodPost, personalSaveBody{DisplayName: " "}, user, http.StatusBadRequest},
		{http.MethodPost, personalSaveBody{DisplayName: "Alice"}, nil, http.StatusUnauthorized},
	} {
		rec, req = settingsRequest(tc.method, "/api/settings/personal", tc.body, tc.user)
		handleSavePersonal(srv, contacts)(rec, req)
		if rec.Code != tc.status {
			t.Fatalf("%s body=%v status=%d want=%d", tc.method, tc.body, rec.Code, tc.status)
		}
	}
}

func TestAppPasswordHTTPWorkflowAndIsolation(t *testing.T) {
	srv := testSettingsServer(t)
	user := createSettingsUser(t, srv)

	rec, req := settingsRequest(http.MethodPost, "/api/settings/app-passwords", map[string]any{
		"name": "  DAV client  ", "scopes": []string{"files", "calendar", "files"},
	}, user)
	handleCreateAppPassword(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("create status=%d body=%s", rec.Code, rec.Body.String())
	}
	var created struct {
		ID     string   `json:"id"`
		Name   string   `json:"name"`
		Token  string   `json:"token"`
		Scopes []string `json:"scopes"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	if created.ID == "" || created.Token == "" || created.Name != "DAV client" || len(created.Scopes) != 2 {
		t.Fatalf("created=%+v", created)
	}

	rec, req = settingsRequest(http.MethodGet, "/api/settings/app-passwords", nil, user)
	handleListAppPasswords(srv)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), created.ID) || strings.Contains(rec.Body.String(), created.Token) {
		t.Fatalf("list status=%d body=%s", rec.Code, rec.Body.String())
	}

	other := &auth.User{ID: "other"}
	rec, req = settingsRequest(http.MethodDelete, "/api/settings/app-passwords/"+created.ID, nil, other)
	req = withChiParam(req, "id", created.ID)
	handleDeleteAppPassword(srv)(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("cross-user delete status=%d", rec.Code)
	}

	rec, req = settingsRequest(http.MethodDelete, "/api/settings/app-passwords/"+created.ID, nil, user)
	req = withChiParam(req, "id", created.ID)
	handleDeleteAppPassword(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("delete status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodDelete, "/api/settings/app-passwords/"+created.ID, nil, user)
	req = withChiParam(req, "id", created.ID)
	handleDeleteAppPassword(srv)(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("second delete status=%d", rec.Code)
	}

	for _, tc := range []struct {
		method string
		body   any
		user   *auth.User
		status int
	}{
		{http.MethodGet, nil, user, http.StatusMethodNotAllowed},
		{http.MethodPost, []byte("{"), user, http.StatusBadRequest},
		{http.MethodPost, map[string]any{"name": "x", "scopes": []string{}}, user, http.StatusBadRequest},
		{http.MethodPost, map[string]any{"name": "x", "scopes": []string{"unknown"}}, user, http.StatusBadRequest},
		{http.MethodPost, map[string]any{"name": "x", "scopes": []string{"files"}}, nil, http.StatusUnauthorized},
	} {
		rec, req = settingsRequest(tc.method, "/api/settings/app-passwords", tc.body, tc.user)
		handleCreateAppPassword(srv)(rec, req)
		if rec.Code != tc.status {
			t.Fatalf("create method=%s body=%v status=%d want=%d", tc.method, tc.body, rec.Code, tc.status)
		}
	}

	rec, req = settingsRequest(http.MethodDelete, "/api/settings/app-passwords/", nil, user)
	handleDeleteAppPassword(srv)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("missing id status=%d", rec.Code)
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/app-passwords", nil, nil)
	handleListAppPasswords(srv)(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous list status=%d", rec.Code)
	}
	rec, req = settingsRequest(http.MethodDelete, "/api/settings/app-passwords/id", nil, nil)
	handleDeleteAppPassword(srv)(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous delete status=%d", rec.Code)
	}
}

func TestHandleAPIMe(t *testing.T) {
	user := &auth.User{
		Username: "alice", Email: "alice@example.test", DisplayName: "Alice",
		AvatarURL: "https://example.test/avatar", OIDCIssuer: "https://issuer.example",
	}
	rec, req := settingsRequest(http.MethodGet, "/api/me", nil, user)
	handleAPIMe(rec, req)
	if rec.Code != http.StatusOK || rec.Header().Get("Content-Type") != "application/json" ||
		!strings.Contains(rec.Body.String(), `"username":"alice"`) || !strings.Contains(rec.Body.String(), `"issuer":"https://issuer.example"`) {
		t.Fatalf("status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodGet, "/api/me", nil, nil)
	handleAPIMe(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous status=%d", rec.Code)
	}
}

func TestFilesAndCalendarSettingsLifecycleValidationAndFailures(t *testing.T) {
	srv := testSettingsServer(t)
	user := createSettingsUser(t, srv)

	filesValue := defaultFilesSettings()
	filesValue.PersonalFolder = "/Documents"
	filesValue.TrashFolder = "/Trash"
	filesValue.DefaultView = "grid"
	rec, req := settingsRequest(http.MethodPost, "/api/settings/files", filesValue, user)
	handleSaveFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("save files settings: %d %s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/files", nil, user)
	handleGetFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"/Documents"`) {
		t.Fatalf("get files settings: %d %s", rec.Code, rec.Body.String())
	}
	for _, value := range []filesSettings{
		{PersonalFolder: "", TrashFolder: "/Trash", DefaultView: "list"},
		{PersonalFolder: "relative", TrashFolder: "/Trash", DefaultView: "list"},
		{PersonalFolder: "/", TrashFolder: "/Trash", DefaultView: "list"},
		{PersonalFolder: "/Documents", TrashFolder: "/", DefaultView: "list"},
		{PersonalFolder: "/Documents", TrashFolder: "/Documents/Trash", DefaultView: "list"},
		{PersonalFolder: "/Documents", TrashFolder: "/Trash", DefaultView: "columns"},
	} {
		rec, req = settingsRequest(http.MethodPost, "/api/settings/files", value, user)
		handleSaveFilesSettings(srv)(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("invalid files settings returned %d: %s", rec.Code, rec.Body.String())
		}
	}
	rec, req = settingsRequest(http.MethodPost, "/api/settings/files", []byte(`{`), user)
	handleSaveFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("malformed files settings=%d", rec.Code)
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/files", nil, user)
	handleSaveFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("files settings method=%d", rec.Code)
	}

	five, bad := 5, 7
	calendarValue := defaultCalendarSettings()
	calendarValue.Timezone = "America/Toronto"
	calendarValue.SlotMinutes = 15
	calendarValue.DefaultTimedReminder = &five
	rec, req = settingsRequest(http.MethodPost, "/api/settings/calendar", calendarValue, user)
	handleSaveCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("save calendar settings: %d %s", rec.Code, rec.Body.String())
	}
	rec, req = settingsRequest(http.MethodGet, "/api/settings/calendar", nil, user)
	handleGetCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "America/Toronto") {
		t.Fatalf("get calendar settings: %d %s", rec.Code, rec.Body.String())
	}
	for _, value := range []calendarSettings{
		{Timezone: "Invalid/Timezone", SlotMinutes: 30},
		{Timezone: "automatic", SlotMinutes: 17},
		{Timezone: "automatic", SlotMinutes: 30, DefaultTimedReminder: &bad},
		{Timezone: "automatic", SlotMinutes: 30, DefaultAllDayReminder: &bad},
	} {
		rec, req = settingsRequest(http.MethodPost, "/api/settings/calendar", value, user)
		handleSaveCalendarSettings(srv)(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("invalid calendar settings returned %d: %s", rec.Code, rec.Body.String())
		}
	}
	rec, req = settingsRequest(http.MethodPost, "/api/settings/calendar", []byte(`{"unknown":true}`), user)
	handleSaveCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("strict calendar settings=%d", rec.Code)
	}

	for _, handler := range []http.HandlerFunc{
		handleGetFilesSettings(srv), handleSaveFilesSettings(srv),
		handleGetCalendarSettings(srv), handleSaveCalendarSettings(srv),
	} {
		rec, req = settingsRequest(http.MethodPost, "/api/settings", filesValue, nil)
		handler(rec, req)
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("unauthorized settings handler=%d", rec.Code)
		}
	}

	closed := testSettingsServer(t)
	closedUser := createSettingsUser(t, closed)
	if err := closed.DB.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		body    any
	}{
		{handleGetTimeFormat(closed), http.MethodGet, nil},
		{handleSaveTimeFormat(closed), http.MethodPost, timeFormatSettings{}},
		{handleGetFilesSettings(closed), http.MethodGet, nil},
		{handleSaveFilesSettings(closed), http.MethodPost, filesValue},
		{handleGetCalendarSettings(closed), http.MethodGet, nil},
		{handleSaveCalendarSettings(closed), http.MethodPost, calendarValue},
		{handleListAppPasswords(closed), http.MethodGet, nil},
		{handleCreateAppPassword(closed), http.MethodPost, map[string]string{"name": "Closed"}},
		{handleDeleteAppPassword(closed), http.MethodDelete, nil},
	} {
		rec, req = settingsRequest(tc.method, "/api/settings", tc.body, closedUser)
		tc.handler(rec, req)
		if rec.Code < 400 {
			t.Errorf("closed DB handler returned %d: %s", rec.Code, rec.Body.String())
		}
	}
}

func withChiParam(req *http.Request, key, value string) *http.Request {
	routeContext := chi.NewRouteContext()
	routeContext.URLParams.Add(key, value)
	return req.WithContext(context.WithValue(req.Context(), chi.RouteCtxKey, routeContext))
}
