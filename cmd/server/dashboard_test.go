package main

import (
	"context"
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
	"zombiezen.com/go/sqlite/sqlitex"
)

type dashboardSettingsFixture struct{}

func (*dashboardSettingsFixture) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: "fixture.widget", Title: "Fixture", Kind: "list", Configurable: true,
		DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(),
	}}
}

func (*dashboardSettingsFixture) DashboardWidget(_ context.Context, request apps.DashboardWidgetRequest, _ string) (apps.WidgetData, error) {
	return apps.WidgetData{Kind: "list", EmptyState: request.Timezone + " " + string(request.Size)}, nil
}

func (*dashboardSettingsFixture) DashboardWidgetSettings(context.Context, apps.DashboardWidgetRequest, string) (apps.WidgetSettings, error) {
	return apps.WidgetSettings{Fields: []apps.WidgetSettingField{{Name: "choice", Type: "text", Value: "old"}}}, nil
}

func (*dashboardSettingsFixture) SaveDashboardWidgetSettings(_ context.Context, _ apps.DashboardWidgetRequest, _ string, values map[string]string) (apps.WidgetSettings, error) {
	return apps.WidgetSettings{Fields: []apps.WidgetSettingField{{Name: "choice", Type: "text", Value: values["choice"]}}}, nil
}

func dashboardRequest(target string, body string) *http.Request {
	request := httptest.NewRequest(map[bool]string{true: http.MethodPut, false: http.MethodGet}[body != ""], target, strings.NewReader(body))
	request = request.WithContext(context.WithValue(request.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	return request
}

func TestDashboardWidgetTimezoneAndSettingsAPI(t *testing.T) {
	registry := apps.NewRegistry()
	registry.RegisterDashboardProvider(1, &dashboardSettingsFixture{})
	router := chi.NewRouter()
	router.Get("/api/dashboard/widgets/{id}", handleDashboardWidget(registry))
	router.Get("/api/dashboard/widgets/{id}/settings", handleDashboardWidgetSettings(registry))
	router.Put("/api/dashboard/widgets/{id}/settings", handleDashboardWidgetSettings(registry))

	invalid := httptest.NewRecorder()
	router.ServeHTTP(invalid, dashboardRequest("/api/dashboard/widgets/fixture.widget?timezone=Not/AZone", ""))
	if invalid.Code != http.StatusBadRequest {
		t.Fatalf("invalid timezone status = %d", invalid.Code)
	}
	valid := httptest.NewRecorder()
	router.ServeHTTP(valid, dashboardRequest("/api/dashboard/widgets/fixture.widget?timezone=America/Toronto", ""))
	if valid.Code != http.StatusOK || !strings.Contains(valid.Body.String(), "America/Toronto standard") {
		t.Fatalf("widget status=%d body=%s", valid.Code, valid.Body.String())
	}
	compact := httptest.NewRecorder()
	router.ServeHTTP(compact, dashboardRequest("/api/dashboard/widgets/fixture.widget?timezone=UTC&size=compact", ""))
	if compact.Code != http.StatusOK || !strings.Contains(compact.Body.String(), "UTC compact") {
		t.Fatalf("compact widget status=%d body=%s", compact.Code, compact.Body.String())
	}
	unsupported := httptest.NewRecorder()
	router.ServeHTTP(unsupported, dashboardRequest("/api/dashboard/widgets/fixture.widget?timezone=UTC&size=giant", ""))
	if unsupported.Code != http.StatusBadRequest {
		t.Fatalf("unsupported size status=%d body=%s", unsupported.Code, unsupported.Body.String())
	}
	save := httptest.NewRecorder()
	router.ServeHTTP(save, dashboardRequest("/api/dashboard/widgets/fixture.widget/settings?timezone=UTC", `{"values":{"choice":"new"}}`))
	if save.Code != http.StatusOK || !strings.Contains(save.Body.String(), `"value":"new"`) {
		t.Fatalf("settings status=%d body=%s", save.Code, save.Body.String())
	}
}

func TestDefaultDashboardPacksAcrossTwelveColumns(t *testing.T) {
	registry := apps.NewRegistry()
	for i := range 4 {
		registry.RegisterDashboardProvider(i, &dashboardSettingsFixtureWithID{id: string(rune('a' + i))})
	}
	layout := defaultDashboard(registry).Layout
	if len(layout) != 4 || layout[0].X != 0 || layout[1].X != 4 || layout[2].X != 8 || layout[3].X != 0 || layout[3].Y != 3 {
		t.Fatalf("layout = %#v", layout)
	}
	for _, item := range layout {
		if item.Size != apps.WidgetStandard {
			t.Fatalf("default item %#v is not standard", item)
		}
	}
}

type dashboardSettingsFixtureWithID struct {
	dashboardSettingsFixture
	id string
}

func (f *dashboardSettingsFixtureWithID) DashboardWidgets() []apps.WidgetDefinition {
	definitions := f.dashboardSettingsFixture.DashboardWidgets()
	definitions[0].ID = f.id
	return definitions
}

func TestDashboardLegacySizesNormalizeAndReflow(t *testing.T) {
	registry := apps.NewRegistry()
	for i := range 3 {
		registry.RegisterDashboardProvider(i, &dashboardSettingsFixtureWithID{id: string(rune('a' + i))})
	}
	normalized, changed := normalizeStoredDashboard(dashboardLayout{Layout: []dashboardItem{
		{ID: "a", X: 0, Y: 0, LegacyW: 3, LegacyH: 3},
		{ID: "b", X: 0, Y: 0, LegacyW: 4, LegacyH: 4},
		{ID: "c", X: 40, Y: -1, Size: apps.WidgetWide},
		{ID: "missing", X: 0, Y: 0, LegacyW: 4, LegacyH: 4},
	}}, registry)
	if !changed {
		t.Fatal("legacy layout was not marked changed")
	}
	if len(normalized.Layout) != 3 {
		t.Fatalf("normalized layout = %#v", normalized.Layout)
	}
	sizes := map[string]apps.WidgetSize{}
	for _, item := range normalized.Layout {
		sizes[item.ID] = item.Size
		if item.LegacyW != 0 || item.LegacyH != 0 || item.X < 0 || item.X >= dashboardColumns || item.Y < 0 {
			t.Fatalf("invalid normalized item %#v", item)
		}
	}
	if sizes["a"] != apps.WidgetCompact || sizes["b"] != apps.WidgetStandard || sizes["c"] != apps.WidgetWide {
		t.Fatalf("normalized sizes = %#v", sizes)
	}
}

func TestDashboardActiveWidgetWinsCollisionAndOthersCompact(t *testing.T) {
	registry := apps.NewRegistry()
	for i := range 3 {
		registry.RegisterDashboardProvider(i, &dashboardSettingsFixtureWithID{id: string(rune('a' + i))})
	}
	layout := reflowDashboard(dashboardLayout{Layout: []dashboardItem{
		{ID: "a", X: 0, Y: 0, Size: apps.WidgetStandard},
		{ID: "b", X: 0, Y: 0, Size: apps.WidgetStandard},
		{ID: "c", X: 4, Y: 8, Size: apps.WidgetStandard},
	}}, registry, "b")
	byID := map[string]dashboardItem{}
	for _, item := range layout.Layout {
		byID[item.ID] = item
	}
	if byID["b"].Y != 0 || byID["a"].Y != 3 || byID["c"].Y != 0 {
		t.Fatalf("reflowed layout = %#v", layout.Layout)
	}
}

func TestDashboardLayoutValidationAndStrictJSON(t *testing.T) {
	registry := apps.NewRegistry()
	registry.RegisterDashboardProvider(1, &dashboardSettingsFixture{})
	if err := validateDashboardLayout(dashboardLayout{Layout: []dashboardItem{
		{ID: "fixture.widget", X: 0, Y: 0, Size: apps.WidgetStandard},
		{ID: "fixture.widget", X: 4, Y: 0, Size: apps.WidgetStandard},
	}}, registry); err == nil {
		t.Fatal("duplicate widget layout was accepted")
	}
	if err := validateDashboardLayout(dashboardLayout{Layout: []dashboardItem{
		{ID: "fixture.widget", X: 0, Y: 0, Size: "giant"},
	}}, registry); err == nil {
		t.Fatal("unsupported widget size was accepted")
	}

	handler := handleDashboardLayout(nil, registry)
	for _, body := range []string{
		`{"layout":[],"extra":true}`,
		`{"layout":[]} {"layout":[]}`,
		`{"layout":[{"id":"fixture.widget","x":0,"y":0,"size":"standard","w":4}]}`,
	} {
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, dashboardRequest("/api/dashboard/layout", body))
		if recorder.Code != http.StatusBadRequest {
			t.Fatalf("body %q status=%d response=%s", body, recorder.Code, recorder.Body.String())
		}
	}
}

func TestDashboardLayoutPersistsNormalizesAndServesCatalog(t *testing.T) {
	srv := testSettingsServer(t)
	registry := apps.NewRegistry()
	registry.RegisterDashboardProvider(1, &dashboardSettingsFixture{})

	get := httptest.NewRecorder()
	handleDashboardLayout(srv, registry)(get, dashboardRequest("/api/dashboard/layout", ""))
	if get.Code != http.StatusOK || !strings.Contains(get.Body.String(), `"fixture.widget"`) {
		t.Fatalf("default status=%d body=%s", get.Code, get.Body.String())
	}

	put := httptest.NewRecorder()
	handleDashboardLayout(srv, registry)(put, dashboardRequest(
		"/api/dashboard/layout",
		`{"layout":[{"id":"fixture.widget","x":11,"y":9,"size":"compact"}],"activeId":"fixture.widget"}`,
	))
	if put.Code != http.StatusOK {
		t.Fatalf("save status=%d body=%s", put.Code, put.Body.String())
	}
	if strings.Contains(put.Body.String(), `"activeId"`) {
		t.Fatalf("transient active id was persisted: %s", put.Body.String())
	}

	get = httptest.NewRecorder()
	handleDashboardLayout(srv, registry)(get, dashboardRequest("/api/dashboard/layout", ""))
	if get.Code != http.StatusOK || !strings.Contains(get.Body.String(), `"x":9`) || !strings.Contains(get.Body.String(), `"y":0`) {
		t.Fatalf("persisted status=%d body=%s", get.Code, get.Body.String())
	}

	catalog := httptest.NewRecorder()
	handleDashboardCatalog(registry)(catalog, dashboardRequest("/api/dashboard/catalog", ""))
	if catalog.Code != http.StatusOK || !strings.Contains(catalog.Body.String(), `"presets"`) || !strings.Contains(catalog.Body.String(), `"fixture.widget"`) {
		t.Fatalf("catalog status=%d body=%s", catalog.Code, catalog.Body.String())
	}

	method := httptest.NewRecorder()
	request := dashboardRequest("/api/dashboard/layout", "")
	request.Method = http.MethodPost
	handleDashboardLayout(srv, registry)(method, request)
	if method.Code != http.StatusMethodNotAllowed {
		t.Fatalf("method status=%d", method.Code)
	}

	anonymous := httptest.NewRecorder()
	load := httptest.NewRequest(http.MethodGet, "/api/dashboard/layout", nil)
	handleDashboardLayout(srv, registry)(anonymous, load)
	if anonymous.Code != http.StatusInternalServerError {
		t.Fatalf("anonymous status=%d", anonymous.Code)
	}
}

type dashboardErrorFixture struct {
	widgetErr error
	loadErr   error
	saveErr   error
}

func (*dashboardErrorFixture) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: "errors.widget", Title: "Errors", Kind: "list", Configurable: true,
		DefaultSize: apps.WidgetCompact, SupportedSizes: []apps.WidgetSize{apps.WidgetCompact},
	}}
}

func (f *dashboardErrorFixture) DashboardWidget(context.Context, apps.DashboardWidgetRequest, string) (apps.WidgetData, error) {
	return apps.WidgetData{}, f.widgetErr
}

func (f *dashboardErrorFixture) DashboardWidgetSettings(context.Context, apps.DashboardWidgetRequest, string) (apps.WidgetSettings, error) {
	return apps.WidgetSettings{}, f.loadErr
}

func (f *dashboardErrorFixture) SaveDashboardWidgetSettings(context.Context, apps.DashboardWidgetRequest, string, map[string]string) (apps.WidgetSettings, error) {
	return apps.WidgetSettings{}, f.saveErr
}

func TestDashboardHandlerFailureAndValidationMatrix(t *testing.T) {
	fixture := &dashboardErrorFixture{}
	registry := apps.NewRegistry()
	registry.RegisterDashboardProvider(1, fixture)
	router := chi.NewRouter()
	router.Get("/api/dashboard/widgets/{id}", handleDashboardWidget(registry))
	router.MethodFunc(http.MethodGet, "/api/dashboard/widgets/{id}/settings", handleDashboardWidgetSettings(registry))
	router.MethodFunc(http.MethodPut, "/api/dashboard/widgets/{id}/settings", handleDashboardWidgetSettings(registry))
	router.MethodFunc(http.MethodPost, "/api/dashboard/widgets/{id}/settings", handleDashboardWidgetSettings(registry))

	check := func(method, target, body string, want int) {
		t.Helper()
		request := dashboardRequest(target, body)
		request.Method = method
		recorder := httptest.NewRecorder()
		router.ServeHTTP(recorder, request)
		if recorder.Code != want {
			t.Fatalf("%s %s status=%d want=%d body=%s", method, target, recorder.Code, want, recorder.Body.String())
		}
	}

	check(http.MethodGet, "/api/dashboard/widgets/missing", "", http.StatusNotFound)
	for _, tc := range []struct {
		err  error
		want int
	}{
		{&apps.DashboardError{Status: http.StatusTeapot, Err: errors.New("short and stout")}, http.StatusTeapot},
		{errors.New("resource not found"), http.StatusNotFound},
		{errors.New("unknown widget backend"), http.StatusNotFound},
		{errors.New("offline"), http.StatusInternalServerError},
		{&apps.DashboardError{Status: 700, Err: errors.New("bad status")}, http.StatusInternalServerError},
	} {
		fixture.widgetErr = tc.err
		check(http.MethodGet, "/api/dashboard/widgets/errors.widget", "", tc.want)
	}
	fixture.widgetErr = nil

	check(http.MethodPost, "/api/dashboard/widgets/errors.widget/settings", "{}", http.StatusMethodNotAllowed)
	check(http.MethodPut, "/api/dashboard/widgets/errors.widget/settings", `{`, http.StatusBadRequest)
	check(http.MethodPut, "/api/dashboard/widgets/errors.widget/settings", `{"values":null}`, http.StatusBadRequest)
	check(http.MethodGet, "/api/dashboard/widgets/missing/settings", "", http.StatusNotFound)

	for _, tc := range []struct {
		err  error
		want int
	}{
		{&apps.DashboardError{Status: http.StatusConflict, Err: errors.New("conflict")}, http.StatusConflict},
		{errors.New("settings not found"), http.StatusNotFound},
		{errors.New("invalid choice"), http.StatusBadRequest},
		{errors.New("value required"), http.StatusBadRequest},
		{errors.New("unknown choice"), http.StatusBadRequest},
		{errors.New("database offline"), http.StatusInternalServerError},
	} {
		fixture.loadErr = tc.err
		check(http.MethodGet, "/api/dashboard/widgets/errors.widget/settings", "", tc.want)
		fixture.loadErr = nil
		fixture.saveErr = tc.err
		check(http.MethodPut, "/api/dashboard/widgets/errors.widget/settings", `{"values":{"x":"y"}}`, tc.want)
		fixture.saveErr = nil
	}

	for name, layout := range map[string]dashboardLayout{
		"unknown":        {Layout: []dashboardItem{{ID: "missing", Size: apps.WidgetCompact}}},
		"legacy":         {Layout: []dashboardItem{{ID: "errors.widget", Size: apps.WidgetCompact, LegacyW: 1}}},
		"x negative":     {Layout: []dashboardItem{{ID: "errors.widget", X: -1, Size: apps.WidgetCompact}}},
		"x high":         {Layout: []dashboardItem{{ID: "errors.widget", X: dashboardColumns, Size: apps.WidgetCompact}}},
		"y negative":     {Layout: []dashboardItem{{ID: "errors.widget", Y: -1, Size: apps.WidgetCompact}}},
		"y high":         {Layout: []dashboardItem{{ID: "errors.widget", Y: dashboardMaxCoordinate + 1, Size: apps.WidgetCompact}}},
		"active missing": {Layout: []dashboardItem{{ID: "errors.widget", Size: apps.WidgetCompact}}, ActiveID: "missing"},
	} {
		if err := validateDashboardLayout(layout, registry); err == nil {
			t.Errorf("%s layout accepted", name)
		}
	}
	normalized, changed := normalizeStoredDashboard(dashboardLayout{
		Layout: []dashboardItem{
			{ID: "errors.widget", X: -3, Y: dashboardMaxCoordinate + 50, LegacyW: 1, LegacyH: 1},
			{ID: "errors.widget", X: 2, Y: 2, Size: apps.WidgetCompact},
		},
		ActiveID: "errors.widget",
	}, registry)
	if !changed || len(normalized.Layout) != 1 || normalized.Layout[0].X != 0 || normalized.ActiveID != "" {
		t.Fatalf("normalized edge layout=%+v changed=%v", normalized, changed)
	}
	if got := nearestDashboardSize(0, 0, fixture.DashboardWidgets()[0]); got != apps.WidgetCompact || absInt(-3) != 3 || absInt(3) != 3 {
		t.Fatal("dashboard size/math helpers failed")
	}
	if !collides(dashboardItem{ID: "missing", Size: "bad"}, nil, dashboardDefinitions(registry)) {
		t.Fatal("invalid widget was considered non-colliding")
	}
}

func TestDashboardStorageRepairAndClosedDatabaseFailures(t *testing.T) {
	srv := testSettingsServer(t)
	registry := apps.NewRegistry()
	registry.RegisterDashboardProvider(1, &dashboardSettingsFixture{})
	ctx := context.Background()
	conn, release, err := srv.DB.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := preferences.SetJSON(conn, "alice", "dashboard", "layout", dashboardLayout{
		Layout: []dashboardItem{{ID: "fixture.widget", X: 99, Y: -10, Size: apps.WidgetStandard}},
	}); err != nil {
		release()
		t.Fatal(err)
	}
	release()
	request := dashboardRequest("/api/dashboard/layout", "")
	loaded, err := loadDashboard(request, srv, registry)
	if err != nil || len(loaded.Layout) != 1 || loaded.Layout[0].X != 8 || loaded.Layout[0].Y != 0 {
		t.Fatalf("normalized stored dashboard=%+v err=%v", loaded, err)
	}

	conn, release, err = srv.DB.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := sqlitex.Execute(conn,
		`UPDATE user_preferences SET value_json = 'not-json' WHERE user_id = ? AND app = 'dashboard' AND key = 'layout'`,
		&sqlitex.ExecOptions{Args: []any{"alice"}},
	); err != nil {
		release()
		t.Fatal(err)
	}
	release()
	if repaired, err := loadDashboard(request, srv, registry); err != nil || len(repaired.Layout) != 1 {
		t.Fatalf("repaired dashboard=%+v err=%v", repaired, err)
	}

	if err := srv.DB.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		method, body string
	}{
		{http.MethodGet, ""},
		{http.MethodPut, `{"layout":[{"id":"fixture.widget","x":0,"y":0,"size":"standard"}]}`},
	} {
		req := dashboardRequest("/api/dashboard/layout", tc.body)
		req.Method = tc.method
		recorder := httptest.NewRecorder()
		handleDashboardLayout(srv, registry)(recorder, req)
		if recorder.Code != http.StatusInternalServerError {
			t.Errorf("closed DB %s status=%d body=%s", tc.method, recorder.Code, recorder.Body.String())
		}
	}
}
