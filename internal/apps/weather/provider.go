// Package weather provides Dar's widget-only Open-Meteo dashboard provider.
package weather

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	locationgeo "github.com/anoosa/dar/internal/geocode"
	"github.com/anoosa/dar/internal/preferences"
)

const (
	defaultGeocodeURL    = locationgeo.DefaultURL
	defaultForecastURL   = "https://api.open-meteo.com/v1/forecast"
	weatherPreferenceKey = "settings"
	maxWeatherResponse   = 1 << 20
)

type ContactUpdater interface {
	UpsertMe(context.Context, string, string, string, auth.UserProfile) (string, error)
}

type Config struct {
	DB          *db.DB
	Contacts    ContactUpdater
	Client      *http.Client
	GeocodeURL  string
	ForecastURL string
	Geocoder    *locationgeo.Client
	Now         func() time.Time
}

type Provider struct {
	db          *db.DB
	contacts    ContactUpdater
	client      *http.Client
	geocoder    *locationgeo.Client
	forecastURL string
	now         func() time.Time
	mu          sync.Mutex
	cache       map[string]cachedForecast
	inflight    map[string]*forecastCall
}

type settings struct {
	Unit           string  `json:"unit"`
	LocationSource string  `json:"location_source,omitempty"`
	LocationLabel  string  `json:"location_label,omitempty"`
	Latitude       float64 `json:"latitude,omitempty"`
	Longitude      float64 `json:"longitude,omitempty"`
	Timezone       string  `json:"timezone,omitempty"`
}

type cachedForecast struct {
	data      apps.WidgetWeather
	expiresAt time.Time
}

type forecastCall struct {
	done chan struct{}
}

func New(config Config) *Provider {
	client := config.Client
	if client == nil {
		client = &http.Client{Timeout: 8 * time.Second}
	}
	if config.Geocoder == nil {
		config.Geocoder = locationgeo.New(client, config.GeocodeURL)
	}
	if config.ForecastURL == "" {
		config.ForecastURL = defaultForecastURL
	}
	if config.Now == nil {
		config.Now = time.Now
	}
	return &Provider{
		db: config.DB, contacts: config.Contacts, client: client,
		geocoder: config.Geocoder, forecastURL: config.ForecastURL, now: config.Now,
		cache: map[string]cachedForecast{}, inflight: map[string]*forecastCall{},
	}
}

func (p *Provider) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: "weather.current", Title: "Weather", Icon: "fa-solid fa-cloud-sun", Kind: "weather",
		DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(),
		EmptyState: "Set a location to see the weather", Configurable: true,
	}}
}

func (p *Provider) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != "weather.current" {
		return apps.WidgetData{}, fmt.Errorf("unknown weather widget")
	}
	user, pref, err := p.userSettings(ctx, request.UserID)
	if err != nil {
		return apps.WidgetData{}, err
	}
	location := strings.TrimSpace(user.Profile.Location)
	if location == "" {
		return apps.WidgetData{
			Kind: "weather", EmptyState: "Set a location to see the weather",
			Actions: []apps.WidgetAction{{ID: "configure", Label: "Set location", Icon: "fa-solid fa-location-dot", Kind: "settings"}},
		}, nil
	}
	if pref.LocationSource != location || pref.LocationLabel == "" {
		resolved, resolveErr := p.geocode(ctx, location)
		if resolveErr != nil {
			return apps.WidgetData{}, resolveErr
		}
		pref.LocationSource = location
		pref.LocationLabel = resolved.Label
		pref.Latitude = resolved.Latitude
		pref.Longitude = resolved.Longitude
		pref.Timezone = resolved.Timezone
		if err := p.saveSettings(ctx, request.UserID, pref); err != nil {
			return apps.WidgetData{}, err
		}
	}
	unit := effectiveUnit(pref.Unit, user.Profile.Locale)
	forecast, err := p.forecast(ctx, pref, unit)
	if err != nil {
		return apps.WidgetData{}, err
	}
	forecastDays := map[apps.WidgetSize]int{
		apps.WidgetCompact: 0, apps.WidgetStandard: 3, apps.WidgetWide: 5, apps.WidgetLarge: 5,
	}[request.Size]
	if len(forecast.Forecast) > forecastDays {
		forecast.Forecast = forecast.Forecast[:forecastDays]
	}
	return apps.WidgetData{Kind: "weather", Weather: &forecast, EmptyState: "Weather unavailable"}, nil
}

func (p *Provider) DashboardWidgetSettings(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetSettings, error) {
	if id != "weather.current" {
		return apps.WidgetSettings{}, fmt.Errorf("widget settings not found")
	}
	user, pref, err := p.userSettings(ctx, request.UserID)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	unit := pref.Unit
	if unit == "" {
		unit = "auto"
	}
	return apps.WidgetSettings{Fields: []apps.WidgetSettingField{
		{Name: "location", Label: "Location", Type: "text", Value: user.Profile.Location, Required: true, Placeholder: "City, country", Help: "This also updates Personal info and your Contacts “Me” card."},
		{Name: "unit", Label: "Temperature unit", Type: "select", Value: unit, Options: []apps.WidgetSettingOption{{Value: "auto", Label: "Automatic"}, {Value: "celsius", Label: "Celsius"}, {Value: "fahrenheit", Label: "Fahrenheit"}}},
	}}, nil
}

func (p *Provider) SaveDashboardWidgetSettings(ctx context.Context, request apps.DashboardWidgetRequest, id string, values map[string]string) (apps.WidgetSettings, error) {
	if id != "weather.current" {
		return apps.WidgetSettings{}, fmt.Errorf("widget settings not found")
	}
	location := strings.TrimSpace(values["location"])
	if location == "" {
		return apps.WidgetSettings{}, fmt.Errorf("location is required")
	}
	if len(location) > 200 {
		return apps.WidgetSettings{}, fmt.Errorf("invalid location")
	}
	unit := strings.TrimSpace(values["unit"])
	if unit == "" {
		unit = "auto"
	}
	if unit != "auto" && unit != "celsius" && unit != "fahrenheit" {
		return apps.WidgetSettings{}, fmt.Errorf("invalid temperature unit")
	}
	resolved, err := p.geocode(ctx, location)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	if p.db == nil {
		return apps.WidgetSettings{}, fmt.Errorf("weather storage unavailable")
	}
	conn, release, err := p.db.Conn(ctx)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	user, err := auth.GetUserByID(conn, request.UserID)
	if err != nil || user == nil {
		release()
		return apps.WidgetSettings{}, fmt.Errorf("user not found")
	}
	user.Profile.Location = resolved.Label
	if err = auth.UpdateUserProfile(conn, user.ID, user.DisplayName, user.Profile); err == nil {
		err = preferences.SetJSON(conn, user.ID, "weather", weatherPreferenceKey, settings{
			Unit: unit, LocationSource: resolved.Label, LocationLabel: resolved.Label,
			Latitude: resolved.Latitude, Longitude: resolved.Longitude, Timezone: resolved.Timezone,
		})
	}
	release()
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	p.clearCache()
	out, err := p.DashboardWidgetSettings(ctx, request, id)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	if p.contacts != nil {
		if _, contactErr := p.contacts.UpsertMe(ctx, user.ID, user.DisplayName, user.Email, user.Profile); contactErr != nil {
			out.Warning = "Location saved, but the Contacts “Me” card could not be updated."
		}
	}
	return out, nil
}

func (p *Provider) userSettings(ctx context.Context, uid string) (*auth.User, settings, error) {
	var pref settings
	if p.db == nil {
		return nil, pref, fmt.Errorf("weather storage unavailable")
	}
	conn, release, err := p.db.Conn(ctx)
	if err != nil {
		return nil, pref, err
	}
	defer release()
	user, err := auth.GetUserByID(conn, uid)
	if err != nil || user == nil {
		return nil, pref, fmt.Errorf("user not found")
	}
	_, err = preferences.GetJSON(conn, uid, "weather", weatherPreferenceKey, &pref)
	return user, pref, err
}

func (p *Provider) saveSettings(ctx context.Context, uid string, pref settings) error {
	conn, release, err := p.db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	return preferences.SetJSON(conn, uid, "weather", weatherPreferenceKey, pref)
}

type resolvedLocation = locationgeo.Location

func (p *Provider) geocode(ctx context.Context, query string) (resolvedLocation, error) {
	location, err := p.geocoder.Resolve(ctx, query)
	if err != nil {
		return resolvedLocation{}, &apps.DashboardError{Status: http.StatusBadGateway, Err: fmt.Errorf("weather location unavailable: %w", err)}
	}
	return location, nil
}

func (p *Provider) forecast(ctx context.Context, pref settings, unit string) (apps.WidgetWeather, error) {
	key := fmt.Sprintf("%.4f,%.4f:%s", pref.Latitude, pref.Longitude, unit)
	p.mu.Lock()
	if cached, ok := p.cache[key]; ok && p.now().Before(cached.expiresAt) {
		p.mu.Unlock()
		return cached.data, nil
	}
	if call := p.inflight[key]; call != nil {
		done := call.done
		p.mu.Unlock()
		select {
		case <-ctx.Done():
			return apps.WidgetWeather{}, ctx.Err()
		case <-done:
			p.mu.Lock()
			cached, ok := p.cache[key]
			p.mu.Unlock()
			if ok {
				return cached.data, nil
			}
			return apps.WidgetWeather{}, fmt.Errorf("weather unavailable")
		}
	}
	call := &forecastCall{done: make(chan struct{})}
	p.inflight[key] = call
	old, hadOld := p.cache[key]
	p.mu.Unlock()

	data, err := p.fetchForecast(ctx, pref, unit)
	p.mu.Lock()
	if err == nil {
		p.cache[key] = cachedForecast{data: data, expiresAt: p.now().Add(15 * time.Minute)}
	} else if hadOld {
		old.data.Stale = true
		old.expiresAt = p.now().Add(5 * time.Minute)
		p.cache[key] = old
		data = old.data
		err = nil
	}
	delete(p.inflight, key)
	close(call.done)
	p.mu.Unlock()
	return data, err
}

func (p *Provider) fetchForecast(ctx context.Context, pref settings, unit string) (apps.WidgetWeather, error) {
	u, _ := url.Parse(p.forecastURL)
	values := u.Query()
	values.Set("latitude", fmt.Sprintf("%.6f", pref.Latitude))
	values.Set("longitude", fmt.Sprintf("%.6f", pref.Longitude))
	values.Set("timezone", "auto")
	values.Set("forecast_days", "5")
	values.Set("current", "temperature_2m,apparent_temperature,weather_code,is_day")
	values.Set("daily", "weather_code,temperature_2m_max,temperature_2m_min")
	if unit == "fahrenheit" {
		values.Set("temperature_unit", "fahrenheit")
	}
	u.RawQuery = values.Encode()
	var response struct {
		Timezone string `json:"timezone"`
		Current  struct {
			Time                string  `json:"time"`
			Temperature         float64 `json:"temperature_2m"`
			ApparentTemperature float64 `json:"apparent_temperature"`
			WeatherCode         int     `json:"weather_code"`
			IsDay               int     `json:"is_day"`
		} `json:"current"`
		Daily struct {
			Time        []string  `json:"time"`
			WeatherCode []int     `json:"weather_code"`
			Maximum     []float64 `json:"temperature_2m_max"`
			Minimum     []float64 `json:"temperature_2m_min"`
		} `json:"daily"`
	}
	if err := p.getJSON(ctx, u.String(), &response); err != nil {
		return apps.WidgetWeather{}, &apps.DashboardError{Status: http.StatusBadGateway, Err: fmt.Errorf("weather unavailable: %w", err)}
	}
	count := min(5, len(response.Daily.Time), len(response.Daily.WeatherCode), len(response.Daily.Maximum), len(response.Daily.Minimum))
	if count == 0 {
		return apps.WidgetWeather{}, fmt.Errorf("weather unavailable: empty forecast")
	}
	label, icon := weatherCode(response.Current.WeatherCode, response.Current.IsDay != 0)
	out := apps.WidgetWeather{
		Location: pref.LocationLabel, Timezone: response.Timezone, Unit: unit,
		Current:   apps.WidgetWeatherCurrent{Temperature: response.Current.Temperature, Apparent: response.Current.ApparentTemperature, Code: response.Current.WeatherCode, Label: label, Icon: icon, IsDay: response.Current.IsDay != 0},
		UpdatedAt: p.now().Unix(),
	}
	for i := 0; i < count; i++ {
		dayLabel, dayIcon := weatherCode(response.Daily.WeatherCode[i], true)
		out.Forecast = append(out.Forecast, apps.WidgetWeatherDay{Date: response.Daily.Time[i], Low: response.Daily.Minimum[i], High: response.Daily.Maximum[i], Code: response.Daily.WeatherCode[i], Label: dayLabel, Icon: dayIcon})
	}
	return out, nil
}

func (p *Provider) getJSON(ctx context.Context, rawURL string, dst any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return err
	}
	response, err := p.client.Do(req)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return fmt.Errorf("upstream status %d", response.StatusCode)
	}
	reader := io.LimitReader(response.Body, maxWeatherResponse+1)
	data, err := io.ReadAll(reader)
	if err != nil {
		return err
	}
	if len(data) > maxWeatherResponse {
		return fmt.Errorf("upstream response too large")
	}
	if err := json.Unmarshal(data, dst); err != nil {
		return fmt.Errorf("decode upstream response: %w", err)
	}
	return nil
}

func effectiveUnit(value, locale string) string {
	if value == "celsius" || value == "fahrenheit" {
		return value
	}
	parts := strings.FieldsFunc(locale, func(r rune) bool { return r == '-' || r == '_' })
	if len(parts) > 1 && strings.EqualFold(parts[len(parts)-1], "US") {
		return "fahrenheit"
	}
	return "celsius"
}

func weatherCode(code int, day bool) (string, string) {
	switch {
	case code == 0:
		if day {
			return "Clear sky", "fa-solid fa-sun"
		}
		return "Clear sky", "fa-solid fa-moon"
	case code <= 3:
		return "Partly cloudy", "fa-solid fa-cloud-sun"
	case code == 45 || code == 48:
		return "Fog", "fa-solid fa-smog"
	case code >= 51 && code <= 67:
		return "Rain", "fa-solid fa-cloud-rain"
	case code >= 71 && code <= 77:
		return "Snow", "fa-solid fa-snowflake"
	case code >= 80 && code <= 82:
		return "Rain showers", "fa-solid fa-cloud-showers-heavy"
	case code >= 85 && code <= 86:
		return "Snow showers", "fa-solid fa-snowflake"
	case code >= 95:
		return "Thunderstorm", "fa-solid fa-cloud-bolt"
	default:
		return "Cloudy", "fa-solid fa-cloud"
	}
}

func (p *Provider) clearCache() {
	p.mu.Lock()
	p.cache = map[string]cachedForecast{}
	p.mu.Unlock()
}
