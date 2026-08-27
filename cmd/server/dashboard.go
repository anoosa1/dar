package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"reflect"
	"sort"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
)

const (
	dashboardColumns       = 12
	dashboardMaxCoordinate = 10000
)

type dashboardItem struct {
	ID      string          `json:"id"`
	X       int             `json:"x"`
	Y       int             `json:"y"`
	Size    apps.WidgetSize `json:"size,omitempty"`
	LegacyW int             `json:"w,omitempty"`
	LegacyH int             `json:"h,omitempty"`
}
type dashboardLayout struct {
	Layout   []dashboardItem `json:"layout"`
	ActiveID string          `json:"activeId,omitempty"`
}

func defaultDashboard(reg *apps.Registry) dashboardLayout {
	out := dashboardLayout{}
	x, y, rowHeight := 0, 0, 0
	for _, d := range reg.DashboardCatalog() {
		preset, _ := apps.DashboardWidgetPreset(d.DefaultSize)
		if x+preset.W > dashboardColumns {
			x = 0
			y += rowHeight
			rowHeight = 0
		}
		out.Layout = append(out.Layout, dashboardItem{ID: d.ID, X: x, Y: y, Size: d.DefaultSize})
		x += preset.W
		if preset.H > rowHeight {
			rowHeight = preset.H
		}
	}
	return reflowDashboard(out, reg, "")
}

func loadDashboard(r *http.Request, srv *server.Server, reg *apps.Registry) (dashboardLayout, error) {
	u := server.GetUser(r)
	if u == nil {
		return dashboardLayout{}, fmt.Errorf("user not found")
	}
	out := defaultDashboard(reg)
	c, release, err := srv.DB.Conn(r.Context())
	if err != nil {
		return dashboardLayout{}, err
	}
	defer release()
	var stored dashboardLayout
	ok, loadErr := preferences.GetJSON(c, u.ID, "dashboard", "layout", &stored)
	if loadErr != nil {
		if saveErr := preferences.SetJSON(c, u.ID, "dashboard", "layout", out); saveErr != nil {
			return dashboardLayout{}, saveErr
		}
		return out, nil
	}
	if ok {
		normalized, changed := normalizeStoredDashboard(stored, reg)
		out = normalized
		if changed {
			if err := preferences.SetJSON(c, u.ID, "dashboard", "layout", out); err != nil {
				return dashboardLayout{}, err
			}
		}
	}
	return out, nil
}

func saveDashboard(r *http.Request, srv *server.Server, layout dashboardLayout) error {
	u := server.GetUser(r)
	c, release, err := srv.DB.Conn(r.Context())
	if err != nil {
		return err
	}
	defer release()
	layout.ActiveID = ""
	return preferences.SetJSON(c, u.ID, "dashboard", "layout", layout)
}

func dashboardDefinitions(reg *apps.Registry) map[string]apps.WidgetDefinition {
	defs := map[string]apps.WidgetDefinition{}
	for _, d := range reg.DashboardCatalog() {
		defs[d.ID] = d
	}
	return defs
}

func widgetSupportsDashboardSize(def apps.WidgetDefinition, size apps.WidgetSize) bool {
	for _, supported := range def.SupportedSizes {
		if supported == size {
			return true
		}
	}
	return false
}

func normalizeStoredDashboard(layout dashboardLayout, reg *apps.Registry) (dashboardLayout, bool) {
	original := append([]dashboardItem(nil), layout.Layout...)
	defs := dashboardDefinitions(reg)
	seen := map[string]bool{}
	items := make([]dashboardItem, 0, len(layout.Layout))
	for _, item := range layout.Layout {
		d, ok := defs[item.ID]
		if !ok || seen[item.ID] {
			continue
		}
		seen[item.ID] = true
		if !widgetSupportsDashboardSize(d, item.Size) {
			item.Size = nearestDashboardSize(item.LegacyW, item.LegacyH, d)
		}
		if item.X < 0 {
			item.X = 0
		}
		if item.Y < 0 {
			item.Y = 0
		}
		if item.X > dashboardColumns-1 {
			item.X = dashboardColumns - 1
		}
		if item.Y > dashboardMaxCoordinate {
			item.Y = dashboardMaxCoordinate
		}
		item.LegacyW = 0
		item.LegacyH = 0
		items = append(items, item)
	}
	layout.Layout = items
	layout.ActiveID = ""
	layout = reflowDashboard(layout, reg, "")
	return layout, !reflect.DeepEqual(original, layout.Layout)
}

func nearestDashboardSize(w, h int, def apps.WidgetDefinition) apps.WidgetSize {
	if w <= 0 || h <= 0 {
		return def.DefaultSize
	}
	best := def.DefaultSize
	bestDistance, bestArea := int(^uint(0)>>1), int(^uint(0)>>1)
	for _, size := range def.SupportedSizes {
		preset, _ := apps.DashboardWidgetPreset(size)
		distance := absInt(w-preset.W) + absInt(h-preset.H)
		areaDistance := absInt(w*h - preset.W*preset.H)
		if distance < bestDistance || distance == bestDistance && areaDistance < bestArea {
			best, bestDistance, bestArea = size, distance, areaDistance
		}
	}
	return best
}

func absInt(value int) int {
	if value < 0 {
		return -value
	}
	return value
}

func validateDashboardLayout(layout dashboardLayout, reg *apps.Registry) error {
	defs := dashboardDefinitions(reg)
	seen := make(map[string]bool, len(layout.Layout))
	activeFound := layout.ActiveID == ""
	for _, item := range layout.Layout {
		def, ok := defs[item.ID]
		if !ok {
			return fmt.Errorf("unknown widget %q", item.ID)
		}
		if seen[item.ID] {
			return fmt.Errorf("duplicate widget %q", item.ID)
		}
		seen[item.ID] = true
		if item.LegacyW != 0 || item.LegacyH != 0 {
			return fmt.Errorf("legacy dimensions are not accepted")
		}
		if !widgetSupportsDashboardSize(def, item.Size) {
			return fmt.Errorf("unsupported size for widget %q", item.ID)
		}
		if item.X < 0 || item.X >= dashboardColumns || item.Y < 0 || item.Y > dashboardMaxCoordinate {
			return fmt.Errorf("invalid coordinates for widget %q", item.ID)
		}
		if item.ID == layout.ActiveID {
			activeFound = true
		}
	}
	if !activeFound {
		return fmt.Errorf("active widget not found")
	}
	return nil
}

func reflowDashboard(layout dashboardLayout, reg *apps.Registry, activeID string) dashboardLayout {
	defs := dashboardDefinitions(reg)
	items := append([]dashboardItem(nil), layout.Layout...)
	sortDashboardItems(items)
	ordered := make([]dashboardItem, 0, len(items))
	if activeID != "" {
		for _, item := range items {
			if item.ID == activeID {
				ordered = append(ordered, item)
				break
			}
		}
	}
	for _, item := range items {
		if item.ID != activeID {
			ordered = append(ordered, item)
		}
	}
	placed := []dashboardItem{}
	for _, item := range ordered {
		preset, ok := apps.DashboardWidgetPreset(item.Size)
		if !ok || !widgetSupportsDashboardSize(defs[item.ID], item.Size) {
			continue
		}
		item.LegacyW, item.LegacyH = 0, 0
		item.X = min(max(item.X, 0), dashboardColumns-preset.W)
		item.Y = max(item.Y, 0)
		for collides(item, placed, defs) {
			item.Y++
		}
		placed = append(placed, item)
	}
	for i := range placed {
		if placed[i].ID == activeID {
			continue
		}
		for placed[i].Y > 0 {
			candidate := placed[i]
			candidate.Y--
			others := append([]dashboardItem(nil), placed[:i]...)
			others = append(others, placed[i+1:]...)
			if collides(candidate, others, defs) {
				break
			}
			placed[i] = candidate
		}
	}
	sortDashboardItems(placed)
	layout.Layout = placed
	layout.ActiveID = ""
	return layout
}

func sortDashboardItems(items []dashboardItem) {
	sort.SliceStable(items, func(i, j int) bool {
		if items[i].Y == items[j].Y {
			return items[i].X < items[j].X
		}
		return items[i].Y < items[j].Y
	})
}

func collides(item dashboardItem, placed []dashboardItem, defs map[string]apps.WidgetDefinition) bool {
	itemPreset, ok := apps.DashboardWidgetPreset(item.Size)
	if !ok || !widgetSupportsDashboardSize(defs[item.ID], item.Size) {
		return true
	}
	for _, p := range placed {
		preset, ok := apps.DashboardWidgetPreset(p.Size)
		if !ok {
			continue
		}
		if item.X < p.X+preset.W && item.X+itemPreset.W > p.X && item.Y < p.Y+preset.H && item.Y+itemPreset.H > p.Y {
			return true
		}
	}
	return false
}

func handleDashboardCatalog(reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		writeAPIJSON(w, map[string]any{
			"presets": apps.DashboardWidgetPresets(),
			"widgets": reg.DashboardCatalog(),
		})
	}
}
func handleDashboardLayout(srv *server.Server, reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			layout, err := loadDashboard(r, srv, reg)
			if err != nil {
				http.Error(w, "failed to load layout", http.StatusInternalServerError)
				return
			}
			writeAPIJSON(w, layout)
			return
		}
		if r.Method != http.MethodPut {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		var in dashboardLayout
		decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 128<<10))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&in); err != nil {
			http.Error(w, "invalid layout", http.StatusBadRequest)
			return
		}
		if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
			http.Error(w, "invalid layout", http.StatusBadRequest)
			return
		}
		if err := validateDashboardLayout(in, reg); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		in = reflowDashboard(in, reg, in.ActiveID)
		if err := saveDashboard(r, srv, in); err != nil {
			http.Error(w, "failed to save layout", http.StatusInternalServerError)
			return
		}
		writeAPIJSON(w, in)
	}
}
func handleDashboardWidget(reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		u := server.GetUser(r)
		request, ok := dashboardWidgetRequest(w, r, u.ID)
		if !ok {
			return
		}
		data, err := reg.DashboardWidget(r.Context(), request, chi.URLParam(r, "id"))
		if err != nil {
			var dashboardErr *apps.DashboardError
			if errors.As(err, &dashboardErr) && dashboardErr.Status >= 400 && dashboardErr.Status <= 599 {
				http.Error(w, dashboardErr.Error(), dashboardErr.Status)
			} else if strings.Contains(err.Error(), "not found") || strings.Contains(err.Error(), "unknown") {
				http.Error(w, "widget not found", http.StatusNotFound)
			} else {
				http.Error(w, "widget unavailable", http.StatusInternalServerError)
			}
			return
		}
		writeAPIJSON(w, data)
	}
}

func dashboardWidgetRequest(w http.ResponseWriter, r *http.Request, userID string) (apps.DashboardWidgetRequest, bool) {
	timezone := strings.TrimSpace(r.URL.Query().Get("timezone"))
	if timezone == "" {
		timezone = "UTC"
	}
	if _, err := time.LoadLocation(timezone); err != nil {
		http.Error(w, "invalid timezone", http.StatusBadRequest)
		return apps.DashboardWidgetRequest{}, false
	}
	return apps.DashboardWidgetRequest{
		UserID:   userID,
		Timezone: timezone,
		Size:     apps.WidgetSize(strings.TrimSpace(r.URL.Query().Get("size"))),
	}, true
}

func handleDashboardWidgetSettings(reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		u := server.GetUser(r)
		request, ok := dashboardWidgetRequest(w, r, u.ID)
		if !ok {
			return
		}
		id := chi.URLParam(r, "id")
		var (
			settings apps.WidgetSettings
			err      error
		)
		switch r.Method {
		case http.MethodGet:
			settings, err = reg.DashboardWidgetSettings(r.Context(), request, id)
		case http.MethodPut:
			var body struct {
				Values map[string]string `json:"values"`
			}
			decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 32<<10))
			decoder.DisallowUnknownFields()
			if decoder.Decode(&body) != nil || body.Values == nil {
				http.Error(w, "invalid settings", http.StatusBadRequest)
				return
			}
			settings, err = reg.SaveDashboardWidgetSettings(r.Context(), request, id, body.Values)
		default:
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		if err != nil {
			status := http.StatusInternalServerError
			var dashboardErr *apps.DashboardError
			if errors.As(err, &dashboardErr) && dashboardErr.Status >= 400 && dashboardErr.Status <= 599 {
				status = dashboardErr.Status
			} else if strings.Contains(err.Error(), "not found") {
				status = http.StatusNotFound
			} else if strings.Contains(err.Error(), "invalid") || strings.Contains(err.Error(), "required") || strings.Contains(err.Error(), "unknown") {
				status = http.StatusBadRequest
			}
			http.Error(w, err.Error(), status)
			return
		}
		writeAPIJSON(w, settings)
	}
}
func writeAPIJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}
