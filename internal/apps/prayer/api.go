package prayer

import (
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/server"
)

type timeValues struct {
	Fajr     int64 `json:"fajr"`
	Sunrise  int64 `json:"sunrise"`
	Dhuhr    int64 `json:"dhuhr"`
	Asr      int64 `json:"asr"`
	Maghrib  int64 `json:"maghrib"`
	Isha     int64 `json:"isha"`
	Midnight int64 `json:"midnight"`
}

type nextPrayer struct {
	Name string `json:"name"`
	Time int64  `json:"time"`
}

type timesResponse struct {
	Date        string      `json:"date"`
	Location    string      `json:"location"`
	Timezone    string      `json:"timezone"`
	Method      Method      `json:"method"`
	MethodLabel string      `json:"method_label"`
	AsrSchool   AsrSchool   `json:"asr_school"`
	Times       timeValues  `json:"times"`
	Next        *nextPrayer `json:"next_prayer,omitempty"`
}

func (a *App) handleTimes(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	settings, err := a.loadSettings(r.Context(), user.ID)
	if err != nil {
		writeError(w, 500, "settings could not be loaded")
		return
	}
	if !settings.Configured {
		writeError(w, http.StatusUnprocessableEntity, "Set a prayer location to see prayer times.")
		return
	}
	loc, err := time.LoadLocation(settings.Timezone)
	if err != nil {
		writeError(w, 500, "saved timezone is invalid")
		return
	}
	now := a.now().In(loc)
	date := time.Date(now.Year(), now.Month(), now.Day(), 12, 0, 0, 0, loc)
	if raw := strings.TrimSpace(r.URL.Query().Get("date")); raw != "" {
		date, err = time.ParseInLocation("2006-01-02", raw, loc)
		if err != nil {
			writeError(w, 400, "date must use YYYY-MM-DD")
			return
		}
	}
	calculation := Calculation{Latitude: settings.Latitude, Longitude: settings.Longitude, Location: loc, Method: settings.Method, AsrSchool: settings.AsrSchool}
	values, err := calculation.Calculate(date)
	if err != nil {
		if errors.Is(err, ErrTimesUnavailable) {
			writeError(w, http.StatusUnprocessableEntity, "Prayer times are unavailable for this date and location.")
			return
		}
		writeError(w, 500, "prayer times could not be calculated")
		return
	}
	response := timesResponse{
		Date: date.Format("2006-01-02"), Location: settings.LocationLabel, Timezone: settings.Timezone,
		Method: settings.Method, MethodLabel: methodLabel(settings.Method), AsrSchool: settings.AsrSchool,
		Times: timeValues{Fajr: values.Fajr.Unix(), Sunrise: values.Sunrise.Unix(), Dhuhr: values.Dhuhr.Unix(), Asr: values.Asr.Unix(), Maghrib: values.Maghrib.Unix(), Isha: values.Isha.Unix(), Midnight: values.Midnight.Unix()},
	}
	if response.Date == now.Format("2006-01-02") {
		for _, prayer := range trackedPrayers(values) {
			if !prayer.Scheduled.Before(now) {
				response.Next = &nextPrayer{Name: prayer.Title, Time: prayer.Scheduled.Unix()}
				break
			}
		}
	}
	writeJSON(w, response)
}

func (a *App) handleSettings(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	current, err := a.loadSettings(r.Context(), user.ID)
	if err != nil {
		writeError(w, 500, "settings could not be loaded")
		return
	}
	if r.Method == http.MethodGet {
		if !current.Configured && current.Location == "" {
			current.Location = user.Profile.Location
		}
		writeJSON(w, current)
		return
	}
	var input struct {
		Location  string    `json:"location"`
		Method    Method    `json:"method"`
		AsrSchool AsrSchool `json:"asr_school"`
		TodoSync  bool      `json:"todo_sync"`
	}
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&input); err != nil {
		writeError(w, 400, "invalid settings")
		return
	}
	input.Location = strings.TrimSpace(input.Location)
	if input.Location == "" || len(input.Location) > 200 || !validMethod(input.Method) || (input.AsrSchool != AsrStandard && input.AsrSchool != AsrHanafi) {
		writeError(w, 400, "location, calculation method, or Asr school is invalid")
		return
	}
	if a.geocoder == nil {
		writeError(w, 503, "location lookup is unavailable")
		return
	}
	next := current
	if !current.Configured || !strings.EqualFold(input.Location, current.Location) && !strings.EqualFold(input.Location, current.LocationLabel) {
		resolved, err := a.geocoder.Resolve(r.Context(), input.Location)
		if err != nil {
			writeError(w, 400, err.Error())
			return
		}
		if _, err := time.LoadLocation(resolved.Timezone); err != nil {
			writeError(w, 400, "location timezone is unavailable")
			return
		}
		next.Location, next.LocationLabel = input.Location, resolved.Label
		next.Latitude, next.Longitude, next.Timezone = resolved.Latitude, resolved.Longitude, resolved.Timezone
	}
	next.Method, next.AsrSchool, next.TodoSync = input.Method, input.AsrSchool, input.TodoSync
	next.SyncWarning = ""
	if input.TodoSync && next.TrackingStartedAt == 0 {
		next.TrackingStartedAt = a.now().Unix()
	}
	if err := a.saveSettings(r.Context(), user.ID, next); err != nil {
		writeError(w, 500, "settings could not be saved")
		return
	}
	if current.TodoSync && !input.TodoSync {
		if err := a.disableSync(r.Context(), user.ID); err != nil {
			next.SyncWarning = "Future prayer tasks could not be removed: " + err.Error()
			_ = a.saveSettings(r.Context(), user.ID, next)
		}
	} else if input.TodoSync {
		if err := a.syncUser(r.Context(), user.ID); err != nil {
			next.SyncWarning = "Prayer tasks will be retried automatically: " + err.Error()
			_ = a.saveSettings(r.Context(), user.ID, next)
		} else {
			next, _ = a.loadSettings(r.Context(), user.ID)
		}
	}
	writeJSON(w, next)
}

type analyticsCounts struct {
	OnTime  int `json:"on_time"`
	OffTime int `json:"off_time"`
	Missed  int `json:"missed"`
	Pending int `json:"pending"`
}

type analyticsPrayer struct {
	Name string `json:"name"`
	analyticsCounts
}

type analyticsResponse struct {
	TrackingStartedAt int64             `json:"tracking_started_at,omitempty"`
	CompletionRate    float64           `json:"completion_rate"`
	Totals            analyticsCounts   `json:"totals"`
	ByPrayer          []analyticsPrayer `json:"by_prayer"`
}

func (a *App) handleAnalytics(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	settings, err := a.loadSettings(r.Context(), user.ID)
	if err != nil {
		writeError(w, 500, "settings could not be loaded")
		return
	}
	occurrences, err := a.occurrences(r.Context(), user.ID)
	if err != nil {
		writeError(w, 500, "analytics could not be loaded")
		return
	}
	states := map[string]struct {
		Status string
		At     int64
	}{}
	if a.todo != nil {
		todoStates, stateErr := a.todo.GeneratedTaskStates(r.Context(), user.ID, sourceName)
		if stateErr != nil {
			writeError(w, 500, "Todo completion data could not be loaded")
			return
		}
		for id, state := range todoStates {
			states[id] = struct {
				Status string
				At     int64
			}{Status: state.Status, At: state.CompletedAt}
		}
	}
	response := analyticsResponse{TrackingStartedAt: settings.TrackingStartedAt}
	byName := map[string]*analyticsPrayer{}
	for _, item := range []struct{ key, title string }{{"fajr", "Fajr"}, {"dhuhr", "Dhuhr"}, {"asr", "Asr"}, {"maghrib", "Maghrib"}, {"isha", "Isha"}} {
		response.ByPrayer = append(response.ByPrayer, analyticsPrayer{Name: item.title})
	}
	for i, key := range []string{"fajr", "dhuhr", "asr", "maghrib", "isha"} {
		byName[key] = &response.ByPrayer[i]
	}
	now := a.now().Unix()
	for _, value := range occurrences {
		state := states[value.ExternalID]
		bucket := &response.Totals
		prayerBucket := &byName[value.Prayer].analyticsCounts
		switch {
		case state.Status == "COMPLETED" && state.At > 0 && state.At >= value.Scheduled && state.At <= value.WindowEnd:
			bucket.OnTime++
			prayerBucket.OnTime++
		case state.Status == "COMPLETED" && state.At > 0:
			bucket.OffTime++
			prayerBucket.OffTime++
		case now > value.WindowEnd:
			bucket.Missed++
			prayerBucket.Missed++
		default:
			bucket.Pending++
			prayerBucket.Pending++
		}
	}
	completedOrMissed := response.Totals.OnTime + response.Totals.OffTime + response.Totals.Missed
	if completedOrMissed > 0 {
		response.CompletionRate = float64(response.Totals.OnTime+response.Totals.OffTime) / float64(completedOrMissed)
	}
	writeJSON(w, response)
}

func writeJSON(w http.ResponseWriter, value any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(value)
}

func writeError(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": message})
}
