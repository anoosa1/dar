package weather

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

type contactRecorder struct {
	calls atomic.Int32
	err   error
}

func (c *contactRecorder) UpsertMe(context.Context, string, string, string, auth.UserProfile) (string, error) {
	c.calls.Add(1)
	return "Alice (Me)", c.err
}

func weatherTestDB(t *testing.T) *db.DB {
	t.Helper()
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "weather.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	user, _, err := auth.FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://issuer.test", "alice")
	if err != nil {
		release()
		t.Fatal(err)
	}
	user.Profile.Location = "Toronto, Canada"
	user.Profile.Locale = "en_CA"
	if err := auth.UpdateUserProfile(conn, user.ID, user.DisplayName, user.Profile); err != nil {
		release()
		t.Fatal(err)
	}
	release()
	return database
}

func weatherUserID(t *testing.T, database *db.DB) string {
	t.Helper()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	user, err := auth.GetUserByEmail(conn, "alice@example.test")
	if err != nil {
		t.Fatal(err)
	}
	return user.ID
}

func TestWeatherWidgetGeocodesCachesAndServesStale(t *testing.T) {
	var forecasts atomic.Int32
	var fail atomic.Bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/geocode":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"results":[{"name":"Toronto","admin1":"Ontario","country":"Canada","latitude":43.65,"longitude":-79.38,"timezone":"America/Toronto"}]}`))
		case "/forecast":
			forecasts.Add(1)
			if fail.Load() {
				http.Error(w, "down", http.StatusServiceUnavailable)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"timezone":"America/Toronto","current":{"time":"2026-07-24T12:00","temperature_2m":24.4,"apparent_temperature":25.1,"weather_code":1,"is_day":1},"daily":{"time":["2026-07-24","2026-07-25","2026-07-26","2026-07-27","2026-07-28"],"weather_code":[1,2,3,61,0],"temperature_2m_max":[25,26,24,22,27],"temperature_2m_min":[18,19,17,16,20]}}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	database := weatherTestDB(t)
	now := time.Date(2026, 7, 24, 12, 0, 0, 0, time.UTC)
	provider := New(Config{DB: database, Client: server.Client(), GeocodeURL: server.URL + "/geocode", ForecastURL: server.URL + "/forecast", Now: func() time.Time { return now }})
	request := apps.DashboardWidgetRequest{UserID: weatherUserID(t, database), Timezone: "America/Toronto", Size: apps.WidgetStandard}
	first, err := provider.DashboardWidget(context.Background(), request, "weather.current")
	if err != nil {
		t.Fatal(err)
	}
	if first.Weather == nil || first.Weather.Location != "Toronto, Ontario, Canada" || len(first.Weather.Forecast) != 3 || first.Weather.Unit != "celsius" {
		t.Fatalf("weather = %#v", first.Weather)
	}
	request.Size = apps.WidgetCompact
	compact, err := provider.DashboardWidget(context.Background(), request, "weather.current")
	if err != nil {
		t.Fatal(err)
	}
	if compact.Weather == nil || len(compact.Weather.Forecast) != 0 {
		t.Fatalf("compact weather = %#v", compact.Weather)
	}
	request.Size = apps.WidgetWide
	wide, err := provider.DashboardWidget(context.Background(), request, "weather.current")
	if err != nil {
		t.Fatal(err)
	}
	if wide.Weather == nil || len(wide.Weather.Forecast) != 5 {
		t.Fatalf("wide weather = %#v", wide.Weather)
	}
	if _, err := provider.DashboardWidget(context.Background(), request, "weather.current"); err != nil {
		t.Fatal(err)
	}
	if forecasts.Load() != 1 {
		t.Fatalf("forecast requests = %d, want 1", forecasts.Load())
	}
	now = now.Add(16 * time.Minute)
	fail.Store(true)
	stale, err := provider.DashboardWidget(context.Background(), request, "weather.current")
	if err != nil {
		t.Fatal(err)
	}
	if stale.Weather == nil || !stale.Weather.Stale {
		t.Fatalf("stale weather = %#v", stale.Weather)
	}
}

func TestWeatherSettingsUpdateCanonicalProfile(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"results":[{"name":"Montreal","admin1":"Quebec","country":"Canada","latitude":45.5,"longitude":-73.56,"timezone":"America/Toronto"}]}`))
	}))
	defer server.Close()
	database := weatherTestDB(t)
	contacts := &contactRecorder{}
	provider := New(Config{DB: database, Contacts: contacts, Client: server.Client(), GeocodeURL: server.URL})
	request := apps.DashboardWidgetRequest{UserID: weatherUserID(t, database), Timezone: "America/Toronto", Size: apps.WidgetStandard}
	result, err := provider.SaveDashboardWidgetSettings(context.Background(), request, "weather.current", map[string]string{"location": "Montreal", "unit": "fahrenheit"})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Fields) != 2 || contacts.calls.Load() != 1 {
		t.Fatalf("settings=%#v contact calls=%d", result, contacts.calls.Load())
	}
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	user, err := auth.GetUserByID(conn, request.UserID)
	release()
	if err != nil {
		t.Fatal(err)
	}
	if user.Profile.Location != "Montreal, Quebec, Canada" {
		t.Fatalf("profile location = %q", user.Profile.Location)
	}
}

func TestWeatherGeocodeQualifiedLocation(t *testing.T) {
	var requestedNames []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestedNames = append(requestedNames, r.URL.Query().Get("name"))
		if r.URL.Query().Get("count") != "100" {
			t.Errorf("count = %q, want 100", r.URL.Query().Get("count"))
		}
		_, _ = w.Write([]byte(`{"results":[
			{"name":"Milton","admin1":"California","country":"United States","country_code":"US","latitude":38.03,"longitude":-120.85,"timezone":"America/Los_Angeles"},
			{"name":"Milton","admin1":"Ontario","country":"Canada","country_code":"CA","latitude":43.52,"longitude":-79.88,"timezone":"America/Toronto"}
		]}`))
	}))
	defer server.Close()
	provider := New(Config{Client: server.Client(), GeocodeURL: server.URL})

	for _, test := range []struct {
		query string
		want  string
	}{
		{query: "Milton, CA", want: "Milton, Ontario, Canada"},
		{query: "Milton, ON", want: "Milton, Ontario, Canada"},
		{query: "Milton, California", want: "Milton, California, United States"},
	} {
		resolved, err := provider.geocode(context.Background(), test.query)
		if err != nil {
			t.Fatalf("geocode(%q): %v", test.query, err)
		}
		if resolved.Label != test.want {
			t.Errorf("geocode(%q) = %q, want %q", test.query, resolved.Label, test.want)
		}
	}
	for _, name := range requestedNames {
		if name != "Milton" {
			t.Errorf("requested name = %q, want Milton", name)
		}
	}
}

func TestWeatherWidgetManifestUnitsAndConditionCodes(t *testing.T) {
	provider := New(Config{})
	widgets := provider.DashboardWidgets()
	if len(widgets) != 1 || widgets[0].ID != "weather.current" {
		t.Fatalf("widgets = %+v", widgets)
	}
	for _, tc := range []struct {
		code int
		day  bool
		want string
	}{
		{0, true, "Clear sky"}, {0, false, "Clear sky"}, {3, true, "Partly cloudy"},
		{45, true, "Fog"}, {48, true, "Fog"}, {51, true, "Rain"}, {67, true, "Rain"},
		{71, true, "Snow"}, {77, true, "Snow"}, {80, true, "Rain showers"}, {82, true, "Rain showers"},
		{85, true, "Snow showers"}, {86, true, "Snow showers"}, {95, true, "Thunderstorm"},
		{999, true, "Thunderstorm"}, {40, true, "Cloudy"},
	} {
		description, icon := weatherCode(tc.code, tc.day)
		if description != tc.want || icon == "" {
			t.Errorf("weatherCode(%d,%v) = %q,%q", tc.code, tc.day, description, icon)
		}
	}
	for _, tc := range []struct {
		value, locale, want string
	}{
		{"celsius", "en_US", "celsius"},
		{"fahrenheit", "en_CA", "fahrenheit"},
		{"auto", "en-US", "fahrenheit"},
		{"auto", "en_CA", "celsius"},
		{"", "", "celsius"},
	} {
		if got := effectiveUnit(tc.value, tc.locale); got != tc.want {
			t.Errorf("effectiveUnit(%q,%q) = %q, want %q", tc.value, tc.locale, got, tc.want)
		}
	}
}

func TestWeatherValidationUpstreamAndInflightFailureMatrix(t *testing.T) {
	geocode := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"results":[{"name":"Toronto","country":"Canada","latitude":43.65,"longitude":-79.38,"timezone":"America/Toronto"}]}`))
	}))
	defer geocode.Close()
	provider := New(Config{Client: geocode.Client(), GeocodeURL: geocode.URL})
	request := apps.DashboardWidgetRequest{UserID: "missing", Size: apps.WidgetStandard}
	if _, err := provider.DashboardWidget(context.Background(), request, "missing"); err == nil {
		t.Fatal("unknown weather widget accepted")
	}
	if _, err := provider.DashboardWidget(context.Background(), request, "weather.current"); err == nil {
		t.Fatal("weather widget without storage succeeded")
	}
	if _, err := provider.DashboardWidgetSettings(context.Background(), request, "missing"); err == nil {
		t.Fatal("unknown weather settings accepted")
	}
	if _, err := provider.DashboardWidgetSettings(context.Background(), request, "weather.current"); err == nil {
		t.Fatal("weather settings without storage succeeded")
	}
	for _, values := range []map[string]string{
		{},
		{"location": strings.Repeat("x", 201)},
		{"location": "Toronto", "unit": "kelvin"},
		{"location": "Toronto", "unit": "celsius"},
	} {
		if _, err := provider.SaveDashboardWidgetSettings(context.Background(), request, "weather.current", values); err == nil {
			t.Errorf("invalid settings accepted: %+v", values)
		}
	}
	if _, err := provider.SaveDashboardWidgetSettings(context.Background(), request, "missing", map[string]string{"location": "Toronto"}); err == nil {
		t.Fatal("unknown settings save accepted")
	}

	for _, fixture := range []struct {
		status int
		body   string
	}{
		{http.StatusBadGateway, `{}`},
		{http.StatusOK, `{`},
		{http.StatusOK, strings.Repeat("x", maxWeatherResponse+1)},
	} {
		upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(fixture.status)
			_, _ = w.Write([]byte(fixture.body))
		}))
		var out any
		p := New(Config{Client: upstream.Client()})
		if err := p.getJSON(context.Background(), upstream.URL, &out); err == nil {
			t.Errorf("upstream fixture status=%d unexpectedly succeeded", fixture.status)
		}
		upstream.Close()
	}
	var out any
	if err := provider.getJSON(context.Background(), "http://[", &out); err == nil {
		t.Fatal("invalid upstream URL accepted")
	}

	empty := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"timezone":"UTC","current":{},"daily":{}}`))
	}))
	defer empty.Close()
	emptyProvider := New(Config{Client: empty.Client(), ForecastURL: empty.URL})
	if _, err := emptyProvider.fetchForecast(context.Background(), settings{}, "celsius"); err == nil {
		t.Fatal("empty forecast accepted")
	}

	inflight := New(Config{})
	pref := settings{Latitude: 1, Longitude: 2}
	key := "1.0000,2.0000:celsius"
	call := &forecastCall{done: make(chan struct{})}
	inflight.inflight[key] = call
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := inflight.forecast(ctx, pref, "celsius"); !errors.Is(err, context.Canceled) {
		t.Fatalf("inflight cancellation=%v", err)
	}
	delete(inflight.inflight, key)
	inflight.inflight[key] = &forecastCall{done: make(chan struct{})}
	close(inflight.inflight[key].done)
	if _, err := inflight.forecast(context.Background(), pref, "celsius"); err == nil {
		t.Fatal("completed inflight without cache succeeded")
	}
}
