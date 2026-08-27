// Package geocode resolves user-entered place names to stable coordinates and
// IANA time zones through Open-Meteo's public geocoding service.
package geocode

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
)

const (
	DefaultURL      = "https://geocoding-api.open-meteo.com/v1/search"
	maxResponseSize = 1 << 20
)

type Location struct {
	Label     string  `json:"label"`
	Latitude  float64 `json:"latitude"`
	Longitude float64 `json:"longitude"`
	Timezone  string  `json:"timezone"`
}

type Client struct {
	httpClient *http.Client
	endpoint   string
}

func New(client *http.Client, endpoint string) *Client {
	if client == nil {
		client = http.DefaultClient
	}
	if strings.TrimSpace(endpoint) == "" {
		endpoint = DefaultURL
	}
	return &Client{httpClient: client, endpoint: endpoint}
}

func (c *Client) Resolve(ctx context.Context, query string) (Location, error) {
	name, qualifiers := splitQuery(query)
	if name == "" {
		return Location{}, fmt.Errorf("location is required")
	}
	u, err := url.Parse(c.endpoint)
	if err != nil {
		return Location{}, fmt.Errorf("invalid geocoding endpoint: %w", err)
	}
	values := u.Query()
	values.Set("name", name)
	if len(qualifiers) == 0 {
		values.Set("count", "1")
	} else {
		// Open-Meteo only searches the place name. Fetch more candidates and
		// disambiguate comma-qualified input against region metadata.
		values.Set("count", "100")
	}
	values.Set("language", "en")
	values.Set("format", "json")
	u.RawQuery = values.Encode()

	var response struct {
		Results []struct {
			Name        string  `json:"name"`
			Admin1      string  `json:"admin1"`
			Admin2      string  `json:"admin2"`
			Country     string  `json:"country"`
			CountryCode string  `json:"country_code"`
			Timezone    string  `json:"timezone"`
			Latitude    float64 `json:"latitude"`
			Longitude   float64 `json:"longitude"`
		} `json:"results"`
	}
	if err := c.getJSON(ctx, u.String(), &response); err != nil {
		return Location{}, fmt.Errorf("location lookup unavailable: %w", err)
	}
	if len(response.Results) == 0 {
		return Location{}, fmt.Errorf("invalid location: no matching city")
	}
	resultIndex := 0
	if len(qualifiers) > 0 {
		bestScore := -1
		for i, candidate := range response.Results {
			score, matches := qualifierScore(qualifiers, candidate.CountryCode, candidate.Country, candidate.Admin1, candidate.Admin2)
			if matches && score > bestScore {
				resultIndex, bestScore = i, score
			}
		}
		if bestScore < 0 {
			return Location{}, fmt.Errorf("invalid location: no matching city")
		}
	}
	result := response.Results[resultIndex]
	if result.Timezone == "" {
		return Location{}, fmt.Errorf("invalid location: timezone unavailable")
	}
	parts := []string{result.Name}
	if result.Admin1 != "" && !strings.EqualFold(result.Admin1, result.Name) {
		parts = append(parts, result.Admin1)
	}
	if result.Country != "" {
		parts = append(parts, result.Country)
	}
	return Location{Label: strings.Join(parts, ", "), Latitude: result.Latitude, Longitude: result.Longitude, Timezone: result.Timezone}, nil
}

func (c *Client) getJSON(ctx context.Context, rawURL string, dst any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return err
	}
	response, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return fmt.Errorf("upstream status %d", response.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(response.Body, maxResponseSize+1))
	if err != nil {
		return err
	}
	if len(data) > maxResponseSize {
		return fmt.Errorf("upstream response too large")
	}
	if err := json.Unmarshal(data, dst); err != nil {
		return fmt.Errorf("decode upstream response: %w", err)
	}
	return nil
}

func splitQuery(query string) (string, []string) {
	parts := strings.Split(query, ",")
	name := strings.TrimSpace(parts[0])
	qualifiers := make([]string, 0, len(parts)-1)
	for _, part := range parts[1:] {
		if qualifier := strings.TrimSpace(part); qualifier != "" {
			qualifiers = append(qualifiers, qualifier)
		}
	}
	return name, qualifiers
}

func qualifierScore(qualifiers []string, countryCode, country, admin1, admin2 string) (int, bool) {
	score := 0
	for _, qualifier := range qualifiers {
		normalized := normalize(qualifier)
		regionName, hasRegion := regionAbbreviations[strings.ToUpper(strings.TrimSpace(qualifier))]
		switch {
		case normalized == normalize(countryCode):
			score += 100
		case normalized == normalize(country):
			score += 90
		case normalized == normalize(admin1):
			score += 80
		case normalized == normalize(admin2):
			score += 70
		case hasRegion && normalize(regionName) == normalize(admin1):
			score += 60
		default:
			return 0, false
		}
	}
	return score, true
}

func normalize(value string) string {
	return strings.ToLower(strings.Join(strings.Fields(strings.TrimSpace(value)), " "))
}

var regionAbbreviations = map[string]string{
	"AB": "Alberta", "BC": "British Columbia", "MB": "Manitoba", "NB": "New Brunswick", "NL": "Newfoundland and Labrador", "NS": "Nova Scotia",
	"NT": "Northwest Territories", "NU": "Nunavut", "ON": "Ontario", "PE": "Prince Edward Island", "QC": "Quebec", "SK": "Saskatchewan", "YT": "Yukon",
	"AL": "Alabama", "AK": "Alaska", "AZ": "Arizona", "AR": "Arkansas", "CA": "California", "CO": "Colorado", "CT": "Connecticut", "DE": "Delaware",
	"FL": "Florida", "GA": "Georgia", "HI": "Hawaii", "ID": "Idaho", "IL": "Illinois", "IN": "Indiana", "IA": "Iowa", "KS": "Kansas",
	"KY": "Kentucky", "LA": "Louisiana", "ME": "Maine", "MD": "Maryland", "MA": "Massachusetts", "MI": "Michigan", "MN": "Minnesota", "MS": "Mississippi",
	"MO": "Missouri", "MT": "Montana", "NE": "Nebraska", "NV": "Nevada", "NH": "New Hampshire", "NJ": "New Jersey", "NM": "New Mexico", "NY": "New York",
	"NC": "North Carolina", "ND": "North Dakota", "OH": "Ohio", "OK": "Oklahoma", "OR": "Oregon", "PA": "Pennsylvania", "RI": "Rhode Island",
	"SC": "South Carolina", "SD": "South Dakota", "TN": "Tennessee", "TX": "Texas", "UT": "Utah", "VT": "Vermont", "VA": "Virginia", "WA": "Washington",
	"WV": "West Virginia", "WI": "Wisconsin", "WY": "Wyoming",
}
