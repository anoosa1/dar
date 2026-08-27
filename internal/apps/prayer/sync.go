package prayer

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps/todo"
)

type namedPrayer struct {
	Name       string
	Title      string
	Scheduled  time.Time
	WindowEnds time.Time
}

func trackedPrayers(times Times) []namedPrayer {
	return []namedPrayer{
		{Name: "fajr", Title: "Fajr", Scheduled: times.Fajr, WindowEnds: times.Sunrise},
		{Name: "dhuhr", Title: "Dhuhr", Scheduled: times.Dhuhr, WindowEnds: times.Asr},
		{Name: "asr", Title: "Asr", Scheduled: times.Asr, WindowEnds: times.Maghrib},
		{Name: "maghrib", Title: "Maghrib", Scheduled: times.Maghrib, WindowEnds: times.Isha},
		{Name: "isha", Title: "Isha", Scheduled: times.Isha, WindowEnds: times.Midnight},
	}
}

func (a *App) syncUser(ctx context.Context, userID string) error {
	a.syncMu.Lock()
	defer a.syncMu.Unlock()
	settings, err := a.loadSettings(ctx, userID)
	if err != nil || !settings.TodoSync || !settings.Configured {
		return err
	}
	if a.todo == nil {
		return fmt.Errorf("Todo sync is unavailable")
	}
	loc, err := time.LoadLocation(settings.Timezone)
	if err != nil {
		return fmt.Errorf("invalid prayer timezone: %w", err)
	}
	calculation := Calculation{Latitude: settings.Latitude, Longitude: settings.Longitude, Location: loc, Method: settings.Method, AsrSchool: settings.AsrSchool}
	now := a.now().In(loc)
	start := time.Date(now.Year(), now.Month(), now.Day(), 12, 0, 0, 0, loc)
	minimum := a.now().Unix()
	if settings.TrackingStartedAt > minimum {
		minimum = settings.TrackingStartedAt
	}
	var specs []todo.GeneratedTask
	var occurrences []occurrence
	for dayOffset := 0; dayOffset < 8; dayOffset++ {
		day := start.AddDate(0, 0, dayOffset)
		times, err := calculation.Calculate(day)
		if err != nil {
			return fmt.Errorf("calculate %s: %w", day.Format("2006-01-02"), err)
		}
		date := day.Format("2006-01-02")
		for _, prayer := range trackedPrayers(times) {
			if prayer.Scheduled.Unix() < minimum {
				continue
			}
			externalID := date + ":" + prayer.Name
			specs = append(specs, todo.GeneratedTask{ExternalID: externalID, Title: prayer.Title, StartAt: prayer.Scheduled.Unix(), DueAt: prayer.Scheduled.Unix(), Tags: []string{"prayer"}})
			occurrences = append(occurrences, occurrence{LocalDate: date, Prayer: prayer.Name, ExternalID: externalID, Scheduled: prayer.Scheduled.Unix(), WindowEnd: prayer.WindowEnds.Unix()})
		}
	}
	if _, err := a.todo.ReconcileGeneratedTasks(ctx, userID, sourceName, "Prayers", specs); err != nil {
		return err
	}
	if err := a.upsertOccurrences(ctx, userID, occurrences); err != nil {
		return err
	}
	if settings.SyncWarning != "" {
		settings.SyncWarning = ""
		return a.saveSettings(ctx, userID, settings)
	}
	return nil
}

func (a *App) disableSync(ctx context.Context, userID string) error {
	a.syncMu.Lock()
	defer a.syncMu.Unlock()
	if a.todo == nil {
		return nil
	}
	occurrences, err := a.occurrences(ctx, userID)
	if err != nil {
		return err
	}
	states, err := a.todo.GeneratedTaskStates(ctx, userID, sourceName)
	if err != nil {
		return err
	}
	now := a.now().Unix()
	remove := make([]string, 0)
	for _, value := range occurrences {
		if value.Scheduled < now {
			continue
		}
		state := states[value.ExternalID]
		if state.Status == "COMPLETED" {
			continue
		}
		remove = append(remove, value.ExternalID)
	}
	removed, err := a.todo.RemoveGeneratedTasks(ctx, userID, sourceName, remove)
	if err != nil {
		return err
	}
	removeSet := make(map[string]bool, len(removed))
	for _, externalID := range removed {
		removeSet[externalID] = true
	}
	return a.removeOccurrenceIDs(ctx, userID, removeSet)
}

func validMethod(value Method) bool {
	_, ok := methodPresets[value]
	return ok
}

func methodLabel(value Method) string {
	switch value {
	case MethodMWL:
		return "Muslim World League"
	case MethodISNA:
		return "ISNA"
	case MethodEgyptian:
		return "Egyptian Authority"
	case MethodKarachi:
		return "Karachi"
	default:
		return strings.ToUpper(string(value))
	}
}
